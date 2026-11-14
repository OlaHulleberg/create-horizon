//! The game glue: Create's engine driven through the mod API. Each subsystem is one module;
//! `lib.rs` hands every hook to all of them through `SUBSYSTEMS`.

pub mod contraptions;
pub mod kinetics;
pub mod net;
pub mod persist;
pub mod server;
pub mod states;
pub mod subsystem;
pub mod world;

use create_engine::world::StateTable;
use mod_api::{ClientHost, Host, ServerHost};
use server::ServerLevel;
use std::cell::RefCell;
use std::collections::HashSet;
use subsystem::Subsystem;

/// Every subsystem, in the order their level ticks run.
pub static SUBSYSTEMS: &[&dyn Subsystem] = &[
    // Before the kinetic solve, as the legacy level tick ran them: what a speed change starts
    // acts on the next tick.
    &contraptions::controller::Controller,
    &contraptions::Contraptions,
    &kinetics::Kinetics,
    // After kinetics, whose block entity type it attaches its part to.
    &contraptions::draw::Drawing,
    &contraptions::seats::Seats,
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
    for s in SUBSYSTEMS {
        s.client_reset();
    }
}
