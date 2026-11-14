//! What each part of the glue (kinetics, logistics, processing...) takes part in.
//!
//! `lib.rs` implements each hook once and hands it to every subsystem in `SUBSYSTEMS` order,
//! which is also the order their level ticks run in (Create's own: block entities in the order
//! of the systems that hold them). A subsystem implements only what it needs; the rest are the
//! default no-ops. Adding a moment means adding a method here and its one call in `lib.rs`.

use super::server::ServerLevel;
use mod_api::{ClientHost, Compound, PlayerRef, Registrar, RegistrationContext, ServerHost};

pub trait Subsystem: Sync {
    /// Its registrations: parts, block entity types, payloads, commands. Once per game.
    fn register(&self, _r: &mut dyn Registrar, _cx: &RegistrationContext) {}

    /// Create's world opened on this server thread (the engine's world exists).
    fn world_opened(&self, _host: &mut dyn ServerHost) {}

    /// The world closed: drop everything kept for it.
    fn world_closed(&self) {}

    /// A committed block change in level `dim`, in engine ids (0 for any block not Create's).
    /// At least one of `old` and `new` is Create's. Runs inside the write: change the world from
    /// the level tick, not here.
    fn block_changed(&self, _dim: usize, _pos: [i32; 3], _old: u32, _new: u32) {}

    /// A neighbour of the Create block `state` at `pos` changed. Inside the write, as above.
    fn neighbor_changed(&self, _dim: usize, _pos: [i32; 3], _state: u32) {}

    /// A column came into level `lv` with what was saved for it (`chunk_data_save`), before
    /// its blocks are found (their block entities load after).
    fn chunk_load(&self, _lv: &mut ServerLevel<'_>, _chunk: [i32; 2], _data: &Compound) {}

    /// What to keep for column `chunk` of level `dim`: written into the column's attachment.
    fn chunk_data_save(&self, _lv: &ServerLevel<'_>, _chunk: [i32; 2], _data: &mut Compound) {}

    /// A column left level `dim`.
    fn chunk_unload(&self, _dim: usize, _chunk: [i32; 2]) {}

    /// The part at `pos` of `lv` turns at a new speed (`KineticBlockEntity.onSpeedChanged`), in
    /// this tick's solve.
    fn speed_changed(&self, _lv: &mut ServerLevel<'_>, _pos: [i32; 3], _speed: f32) {}

    /// After a server level's tick.
    fn level_tick(&self, _lv: &mut ServerLevel<'_>) {}

    /// A player joined the server.
    fn player_joined(&self, _host: &mut dyn ServerHost, _player: PlayerRef) {}

    /// Bytes on one of Create's channels from a player's client. True when this subsystem owns
    /// `channel` (the others are not asked).
    fn from_client(
        &self,
        _host: &mut dyn ServerHost,
        _player: PlayerRef,
        _channel: &str,
        _data: &[u8],
    ) -> bool {
        false
    }

    /// Bytes on one of Create's channels from the server, on the client. True when this
    /// subsystem owns `channel`.
    fn from_server(&self, _host: &mut dyn ClientHost, _channel: &str, _data: &[u8]) -> bool {
        false
    }

    /// Each tick of a client's level.
    fn client_tick(&self, _host: &mut dyn ClientHost) {}

    /// The client left its world: drop what it kept.
    fn client_reset(&self) {}
}
