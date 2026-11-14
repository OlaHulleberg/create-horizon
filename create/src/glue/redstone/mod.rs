//! Create's redstone: powered and toggle latches, brass diodes (pulse repeaters, extenders and
//! timers), rose quartz lamps, redstone links, controller rails, analog levers, smart observers,
//! threshold switches and cuckoo clocks; and what comparators read of Create's blocks.
//!
//! Each block answers the game's signal questions from its state (`parts`: `block_signal`,
//! `is_signal_source`, a minecart passing a controller rail); the latches' and diodes' scheduled
//! ticks are taken from the repeater they are built on, and run in the level tick here
//! (`ticks`), as do lamps, links, rails and cuckoo clocks; levers, observers and switches look
//! after the solve (`detectors`); comparators last (`analog`, the `Comparators` subsystem).
//! What clients draw (link frequencies, lever handles, cuckoo hands, doors and figures) is in
//! `client`, sent on the legacy channels.
//!
//! A smart observer's or threshold switch's filter is read, set by hand, saved and tested by the
//! items port's filter helpers (`items::filters`), as the legacy glue's were; the filters of
//! broken detectors (`detectors.dropped`) are dropped there too.
//!
//! Left out, the API having no way to do it:
//! - A redstone link slot's item is drawn as a block item (`isGui3d`) when the item places a
//!   block: the API does not say whether an item model is 3D (`ItemRenderer` `isGui3d`).
//!
//! Not ported yet (the API could do it): the threshold switch's screen (its thresholds are set
//! with `/create_debug threshold`) and the brass diodes' progress indicator
//! (`BrassDiodeRenderer`).

mod analog;
mod client;
mod detectors;
mod parts;
pub(crate) mod pose;
mod saved;
mod ticks;

pub use analog::{Comparators, analog_touched};
pub(crate) use detectors::{item_id, slot_stack};
pub(crate) use parts::with_properties;

use super::server::ServerLevel;
use super::subsystem::Subsystem;
use super::world::{try_with_world, with_world};
use create_engine::Block;
use create_engine::kinetics::Dir;
use create_engine::parts::StateInfo;
use mod_api::{
    BlockState, ClientHost, Compound, Direction, Flow, HostBlockStates, PlayerRef,
    RegisterCommandsEvent, Registrar, RegistrationContext, RunModCommandEvent, ServerHost,
};

pub struct Redstone;

/// Smart observers and threshold switches looking at their targets (legacy `tick_detectors`):
/// late in the level tick, after the machines, belts and arms moved items and before `fluids`'
/// `Tanks` hands on the tick's tank changes, which they read.
pub struct Detectors;

impl Subsystem for Detectors {
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        detectors::tick_detectors(lv);
    }
}

impl Subsystem for Redstone {
    /// A cuckoo clock's hands and doors are drawn each frame (`client`).
    fn animated(
        &self,
        block: create_engine::Block,
        _pos: [i32; 3],
        _entity: &mod_api::ModBlockEntity,
    ) -> bool {
        matches!(
            block,
            create_engine::Block::CuckooClock | create_engine::Block::MysteriousCuckooClock
        )
    }

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        parts::register(r);
        analog::register(r);
        client::register(r);
    }

    fn world_closed(&self) {
        parts::forget();
        analog::forget();
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        saved::load(lv, data);
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        saved::save(lv, chunk, data);
    }

    fn chunk_unload(&self, dim: usize, chunk: [i32; 2]) {
        analog::column_unloaded(dim, chunk);
    }

    fn block_changed(&self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        let block = |s: u32| info(s).map(|(b, _)| b);
        if old == 0 || new == 0 || block(old) != block(new) {
            analog::block_changed(dim, pos);
        }
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        parts::place_detectors(lv);
        ticks::tick_latches(lv);
        ticks::tick_controller_rails(lv);
        ticks::tick_diodes(lv);
        ticks::tick_lamps(lv);
        ticks::tick_cuckoos(lv);
        ticks::tick_links(lv);
        analog::take_touched(lv);
    }

    fn player_joined(&self, host: &mut dyn ServerHost, player: PlayerRef) {
        let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
            return;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        let dim = lv.dim;
        let links: Vec<[i32; 3]> = with_world(|w| {
            w.dims[dim]
                .links
                .links
                .iter()
                .filter(|(_, l)| l.key != create_engine::link::Key::default())
                .map(|(p, _)| *p)
                .collect()
        })
        .unwrap_or_default();
        ticks::send_links(&mut lv, &[player], &links);
        let levers: Vec<[i32; 3]> =
            with_world(|w| w.dims[dim].levers.levers.keys().copied().collect()).unwrap_or_default();
        detectors::send_levers(&mut lv, &[player], &levers);
    }

    fn server_message(&self, host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        client::server_message(host, channel, data)
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        client::tick(host);
    }

    fn client_reset(&self) {
        client::reset();
    }

    fn register_commands(&self, _host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        detectors::register_commands(event);
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        detectors::run_command(host, event)
    }
}

/// The block and state of engine id `state`, read so that a hook asked inside one of the glue's
/// own writes still answers (`try_with_world`).
pub(crate) fn info(state: u32) -> Option<(Block, StateInfo)> {
    try_with_world(|w| w.table.get(state).map(|(b, i)| (b, *i))).flatten()
}

/// The block at `pos` of `lv` with its state, for Create's blocks.
pub(crate) fn block_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(Block, StateInfo)> {
    let state = lv.block(pos);
    if state == 0 {
        return None;
    }
    info(state)
}

/// The API direction of engine direction `d` (both in vanilla's order).
pub(crate) fn direction(d: Dir) -> Direction {
    Direction::ALL[d as usize]
}

/// `pos` moved one block toward `d`.
pub(crate) fn offset(pos: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]
}

impl ServerLevel<'_> {
    /// The block name of `state` (`minecraft:redstone_wire`).
    pub(crate) fn state_name(&self, state: BlockState) -> Option<String> {
        use mod_api::BlockMetadata;
        self.host
            .block_states()
            .name(state.block())
            .ok()
            .map(str::to_owned)
    }

    /// The value of property `key` of `state`.
    pub(crate) fn state_property(&self, state: BlockState, key: &str) -> Option<String> {
        state
            .property(&self.host.block_states(), key)
            .ok()
            .flatten()
            .map(str::to_owned)
    }

    /// Plays `sound` at the centre of `pos` for the players near it (`level.playSound(null, ..,
    /// SoundSource.BLOCKS, ..)`).
    pub(crate) fn play_block_sound(&mut self, sound: &str, pos: [i32; 3], volume: f32, pitch: f32) {
        let at = pos.map(|v| f64::from(v) + 0.5);
        self.level.play_sound(
            &mut *self.host,
            sound,
            at,
            mod_api::SoundSource::Blocks,
            volume,
            pitch,
        );
    }
}
