//! Create's trains and minecart contraptions: tracks and their curves, stations, signals and
//! observers, the trains running between them over the railway graph (the engine's `trains`
//! and `network`), the train controls and their HUD (`train_controls`, `train_hud`), and cart
//! assemblers with the contraptions and couplings of minecarts (`cart`, `mounted`, `coupling`).
//!
//! - [`Trains`] (`railway`, `tracks`, `assembly`, `saved`): the railway's tick, the stations'
//!   assembly and the bogeys railway casing places at them, the track item's placement, the
//!   curves and signal lights clients draw (both in the track blocks' own block entity data),
//!   and the trains' save with their columns.
//! - `visuals`: trains and cart contraptions as players see them, each block a vanilla block
//!   display the server moves with its carriage, bogey or cart (`CarriageContraptionVisual`,
//!   `StandardBogeyVisual`, `OrientedContraptionEntity`'s drawing).
//! - `riding`: carriage seats, a player sitting on an item display stand-in the server keeps on
//!   the seat.
//! - [`TrainControls`] (`controls`, `client`): clicks on carriages, driving by the controls'
//!   keys and the train HUD.
//! - [`Carts`] (`carts`): cart assemblers acting on the carts that pass them, contraptions
//!   riding carts, their items, couplings, and their save with the world.
//!
//! What the mod API cannot do, and is left out (each with the legacy call it replaced):
//!
//! - **Drawing couplings' connectors** (`CouplingRenderer`): the cable between two carts bends
//!   along a curve no block model draws.
//! - **Standing on and colliding with carriages** (`entity_collisions`, `ContraptionCollider`):
//!   the API has collision exclusions (boxes where blocks don't stop a player), not moving
//!   colliders. Needs `notes/moving-structures-note.md`'s level colliders.
//! - **The `create_debug` commands** (`assemble`, `drive`, `schedule`, `station_name`,
//!   `curve`, `train`; `hand_schedule` for the schedule item): the command subsystem's, which
//!   can call `assembly::use_station` and the railway as the legacy did.
//! - **Taking the controls' keys from the game** is done with `movement_input` (the player
//!   neither walks, jumps nor sneaks), as Create's `setDown(false)` does; `input_key` can't
//!   consume a key, so a key bound to something else still does it.

pub mod assembly;
mod carts;
mod client;
mod controls;
mod railway;
mod riding;
mod saved;
mod structure;
mod tracks;
mod visuals;

pub use carts::Carts;
pub use controls::TrainControls;
pub use railway::Trains;

use super::states;
use mod_api::{BlockState, Host, HostBlockStates, PlayerRef, ServerHost, Text};
use std::cell::RefCell;
use std::collections::HashMap;

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
