//! What each part of the glue (kinetics, logistics, processing...) takes part in.
//!
//! `lib.rs` implements each hook once and hands it to every subsystem in `SUBSYSTEMS` order,
//! which is also the order their level ticks run in (Create's own: block entities in the order
//! of the systems that hold them). A subsystem implements only what it needs; the rest are the
//! default no-ops. Adding a moment means adding a method here and its one call in `lib.rs`.
//!
//! The global hooks several subsystems share take the hook's own host and event, so a
//! subsystem reads them as the API documents them: `lib.rs` calls each subsystem in order and
//! stops at the first `Flow::Cancel` (or the first `Some` for a replacement). A moment that
//! belongs to one block, item or block entity is not here: the subsystem attaches its own Part
//! to its own blocks in `register` (a block may carry several Parts).

use super::server::ServerLevel;
use mod_api::{
    AttackEntityEvent, BreakBlockEvent, ClientHost, Compound, EntityInteractEvent,
    EntityMountEvent, EntityPlaceEvent, Flow, HostSide, InputKeyEvent, InteractionResult,
    LeftClickBlockEvent, LevelSaveEvent, MouseScrollingEvent, PlayerRef, PlayerTickPostEvent,
    RegisterCommandsEvent, Registrar, RegistrationContext, RenderGuiPostEvent,
    RightClickBlockEvent, RightClickItemEvent, RunModCommandEvent, ServerHost,
};

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

    /// An item on the belt segment at `segment` reaches its middle (`locked` false:
    /// `BeltProcessingBehaviour.handleReceivedItem`) or is held there (`handleHeldItem`): what
    /// a machine above it does. The first subsystem to answer acts; None when it has none there.
    fn belt_process(
        &self,
        _lv: &mut ServerLevel<'_>,
        _segment: [i32; 3],
        _item: &mut create_engine::belt::Item,
        _locked: bool,
    ) -> Option<create_engine::belt::Process> {
        None
    }

    /// A player joined the server.
    fn player_joined(&self, _host: &mut dyn ServerHost, _player: PlayerRef) {}

    /// Bytes on one of Create's channels from a player's client. True when this subsystem owns
    /// `channel` (the others are not asked).
    fn client_message(
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
    fn server_message(&self, _host: &mut dyn ClientHost, _channel: &str, _data: &[u8]) -> bool {
        false
    }

    /// Each tick of a client's level.
    fn client_tick(&self, _host: &mut dyn ClientHost) {}

    /// The client left its world: drop what it kept.
    fn client_reset(&self) {}

    /// `Hooks::entity_place`: a block is about to be placed.
    fn entity_place(&self, _host: &mut dyn ServerHost, _event: &mut EntityPlaceEvent) -> Flow {
        Flow::Continue
    }

    /// `Hooks::right_click_block`, on either side.
    fn right_click_block(&self, _host: HostSide, _event: &mut RightClickBlockEvent<'_>) -> Flow {
        Flow::Continue
    }

    /// `Hooks::right_click_item`, on either side.
    fn right_click_item(&self, _host: HostSide, _event: &mut RightClickItemEvent<'_>) -> Flow {
        Flow::Continue
    }

    /// `Hooks::left_click_block`.
    fn left_click_block(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut LeftClickBlockEvent,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::break_block`: a player breaks a block.
    fn break_block(&self, _host: &mut dyn ServerHost, _event: &mut BreakBlockEvent<'_>) -> Flow {
        Flow::Continue
    }

    /// `Hooks::entity_interact`: a player right-clicks an entity. The first `Some` wins.
    fn entity_interact(
        &self,
        _host: HostSide,
        _event: &mut EntityInteractEvent<'_>,
    ) -> Option<InteractionResult> {
        None
    }

    /// `Hooks::attack_entity`.
    fn attack_entity(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut AttackEntityEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::entity_mount`: an entity mounts or dismounts.
    fn entity_mount(&self, _host: &mut dyn ServerHost, _event: &mut EntityMountEvent<'_>) -> Flow {
        Flow::Continue
    }

    /// `Hooks::player_tick_post`, on either side.
    fn player_tick(&self, _host: HostSide, _event: &mut PlayerTickPostEvent) {}

    /// `Hooks::register_commands`: push this subsystem's commands.
    fn register_commands(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut RegisterCommandsEvent<'_>,
    ) {
    }

    /// `Hooks::run_mod_command`: `Flow::Cancel` when the command was this subsystem's.
    fn run_mod_command(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::level_save`.
    fn level_save(&self, _host: &mut dyn ServerHost, _event: &mut LevelSaveEvent<'_>) {}

    /// `Hooks::input_key`, on the client.
    fn input_key(&self, _host: &mut dyn ClientHost, _event: &mut InputKeyEvent<'_>) {}

    /// `Hooks::mouse_scrolling`, on the client: `Flow::Cancel` when the scroll was taken.
    fn mouse_scrolling(
        &self,
        _host: &mut dyn ClientHost,
        _event: &mut MouseScrollingEvent,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::render_gui_post`: draw on the HUD.
    fn render_gui_post(&self, _host: &dyn ClientHost, _event: &mut RenderGuiPostEvent<'_>) {}
}
