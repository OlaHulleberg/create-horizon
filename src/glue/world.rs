//! Create's state for the world a server thread runs.
//!
//! One `Hooks` value serves every game in the process: the window's client game and the server
//! thread's game (single player), or one dedicated server. Server hooks run on the server's
//! thread, client hooks on the window's, so the world lives in a thread-local: each game has
//! its own, and a world that closes drops it.

use create_engine::world::{CreateWorld, dimension_index};
use mod_api::Host;
use std::cell::RefCell;

thread_local! {
    static WORLD: RefCell<Option<CreateWorld>> = const { RefCell::new(None) };
    /// The levels this world has opened, by index: the vanilla three first, then mod
    /// dimensions in the order they opened.
    static LEVELS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Opens Create's world for this server's world. False, and the mod stays idle, when Create's
/// blocks are not in the game.
pub fn open(host: &dyn Host) -> bool {
    let Some(table) = super::states::open(host) else {
        eprintln!("[create] no Create block states: the mod stays idle");
        return false;
    };
    eprintln!(
        "[create] {} Create block states from id {}",
        table.len(),
        table.first()
    );
    WORLD.with_borrow_mut(|w| *w = Some(CreateWorld::new(table)));
    LEVELS.with_borrow_mut(|l| {
        l.clear();
        l.extend(
            [
                "minecraft:overworld",
                "minecraft:the_nether",
                "minecraft:the_end",
            ]
            .map(String::from),
        );
    });
    true
}

/// The world closed (or failed to open): forget all of it.
pub fn close() {
    WORLD.with_borrow_mut(|w| *w = None);
    LEVELS.with_borrow_mut(Vec::clear);
}

/// Whether Create's world is open on this thread.
pub fn is_open() -> bool {
    WORLD.with_borrow(Option::is_some)
}

/// Runs `f` on the open world; `None` when there is none (no world, or Create's content off).
/// `f` must not call back into `with_world`: hooks that fire inside a write are handled after it.
pub fn with_world<R>(f: impl FnOnce(&mut CreateWorld) -> R) -> Option<R> {
    WORLD.with_borrow_mut(|w| w.as_mut().map(f))
}

/// The engine's index of level `dimension` (`minecraft:overworld`), giving a mod dimension the
/// next one the first time it is asked.
pub fn level_index(dimension: &str) -> Option<usize> {
    if !is_open() {
        return None;
    }
    if let Some(i) = dimension_index(dimension) {
        return Some(i);
    }
    let i = LEVELS.with_borrow_mut(|l| match l.iter().position(|d| d == dimension) {
        Some(i) => i,
        None => {
            l.push(dimension.to_owned());
            l.len() - 1
        }
    });
    with_world(|w| w.ensure_dimension(i));
    Some(i)
}

/// The id of level index `dim`.
pub fn level_id(dim: usize) -> Option<String> {
    LEVELS.with_borrow(|l| l.get(dim).cloned())
}
