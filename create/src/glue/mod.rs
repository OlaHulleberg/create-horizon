//! The game glue: Create's engine driven through the mod API. Each subsystem is one module;
//! `lib.rs` hands every hook to all of them through `SUBSYSTEMS`.

pub(crate) mod advancements;
pub mod blocks;
pub mod contraptions;
pub mod copycat;
pub mod decor;
pub mod display;
pub(crate) mod drawn;
pub mod equipment;
pub mod fluids;
pub mod gauges;
pub mod interaction;
pub mod items;
pub mod kinetics;
#[macro_use]
pub mod log;
pub mod logistics;
pub mod models;
pub mod net;
pub mod packages;
pub mod persist;
pub mod pipelines;
pub mod processing;
pub mod redstone;
pub mod schematics;
pub mod sequencer;
pub mod server;
pub mod sources;
pub mod states;
pub mod stock;
pub mod subsystem;
pub mod trains;
pub mod transmission;
pub mod world;

use create_engine::world::StateTable;
use mod_api::{ClientHost, Host, ServerHost};
use server::ServerLevel;
use std::cell::RefCell;
use std::collections::HashSet;
use subsystem::Subsystem;

/// Every subsystem, in the order their level ticks run.
///
/// Before the kinetic solve, so that what they change turns in this tick's solve:
/// - `Equipment`: backtanks fill (first, ahead of Create's own tick).
/// - `Kinetics`: cranks and valves turn down, water wheels, clutches and gearshifts follow
///   redstone, chain drives, sequenced gearshifts. It declares the `create:kinetic` block entity
///   type in `register`, so whatever attaches to `kinetics::kind()` there comes after it.
/// - `Blocks`: sliding doors (postboxes open and shut at their menu, not here).
/// - `Redstone`: latches, controller rails, diodes, lamps, cuckoo clocks, links, then the
///   contents touched since the last tick.
/// - `Controller` (linked controllers), `Display` (display links), `Contraptions` (bearings,
///   their actors, gantries, windmills, stickers, pistons, mounted interfaces, lifts) and its
///   `Drawing`.
/// - `Processing`: crafters, millstones, fans, crushers, blaze burners, mixers.
/// - `Fluids`: pipes and pumps, boilers, valves and hoses, tank states, spouts, drains.
/// - `Carts`, then `Trains` (the controls, the trains, the HUD, curves) and `TrainControls`.
/// - `Logistics`: vaults, chutes, funnels, tunnels, brass tunnels, depots, belts (whose
///   machines answer `belt_process`), and `Entities`, its part of the kinetic type.
/// - `processing::Tools`: drills, saws, then presses and deployers on what the depots and belts
///   did this tick.
/// - `Arms`: mechanical arms, weighted ejectors.
/// - `redstone::Detectors`: smart observers and threshold switches, reading this tick's tank
///   changes before `fluids::Tanks` hands them on (and drains them).
/// - `kinetics::Solve`: belts settle, the network is solved, speeds go to clients.
///
/// After the solve:
/// - `Gauges`: dials the solve moved.
/// - `Packages`, `Stock`, `Schematics`, `Decor`, `Seats`, `Copycats`, `Items`: the
///   subsystems that tick after Create's own.
/// - `Comparators`, last: comparators beside every block whose output changed read it again.
///
/// The same order is the order the shared moments ask them in: the wheel goes to the train
/// controls before the value boxes; a left click to decor before the items; a placement to the
/// equipment before the items; the kinetic world hears of a block change before the others.
pub static SUBSYSTEMS: &[&dyn Subsystem] = &[
    &equipment::Equipment,
    &items::potato::PotatoCannon,
    &kinetics::Kinetics,
    &blocks::Blocks,
    &redstone::Redstone,
    &contraptions::controller::Controller,
    &display::Display,
    &contraptions::Contraptions,
    &contraptions::draw::Drawing,
    &processing::Processing,
    &fluids::Fluids,
    &trains::Carts,
    &trains::Trains,
    &trains::TrainControls,
    &interaction::Interaction,
    &logistics::Logistics,
    &logistics::Entities,
    &processing::Tools,
    &logistics::Arms,
    &redstone::Detectors,
    &fluids::Tanks,
    &kinetics::Solve,
    &gauges::Gauges,
    &sources::Sources,
    &packages::Packages,
    &stock::Stock,
    &schematics::Schematics,
    &decor::Decor,
    &contraptions::seats::Seats,
    &copycat::Copycats,
    &items::Items,
    &redstone::Comparators,
    &advancements::Owners,
];

thread_local! {
    /// The columns of each level whose saved data the subsystems have been given.
    static COLUMNS: RefCell<HashSet<(usize, [i32; 2])>> = RefCell::new(HashSet::new());
    /// The client's copy of the engine's state table, for drawing (a client of a remote server
    /// has no server thread of its own to build it).
    static CLIENT_TABLE: RefCell<Option<StateTable>> = const { RefCell::new(None) };
}

/// Gives the subsystems column `chunk`'s saved data, once while it is loaded: when the column
/// loads, or earlier when one of its block entities loads first.
pub fn ensure_column(host: &mut dyn ServerHost, dimension: &str, chunk: [i32; 2]) {
    let Some(mut lv) = ServerLevel::new(host, dimension) else {
        return;
    };
    if !COLUMNS.with_borrow_mut(|c| c.insert((lv.dim, chunk))) {
        return;
    }
    let data = persist::read(&mut lv, chunk);
    for s in SUBSYSTEMS {
        s.chunk_load(&mut lv, chunk, &data);
    }
}

/// Column `chunk` of level `dim` left: the next load reads its data again.
pub fn forget_column(dim: usize, chunk: [i32; 2]) {
    COLUMNS.with_borrow_mut(|c| c.remove(&(dim, chunk)));
}

/// Forgets every column (the world closed).
pub fn forget_columns() {
    COLUMNS.with_borrow_mut(HashSet::clear);
}

/// Runs `f` on the client's state table, building it on first use. `None` when Create's blocks
/// are not in the game.
pub fn client_table<R>(host: &dyn Host, f: impl FnOnce(&StateTable) -> R) -> Option<R> {
    CLIENT_TABLE.with_borrow_mut(|t| {
        if t.is_none() {
            *t = states::open(host);
        }
        t.as_ref().map(f)
    })
}

/// The client left its world.
pub fn client_reset(_host: &mut dyn ClientHost) {
    CLIENT_TABLE.with_borrow_mut(|t| *t = None);
    models::forget();
    for s in SUBSYSTEMS {
        s.client_reset();
    }
}
