//! Create's redstone: powered and toggle latches, brass diodes (pulse repeaters, extenders and
//! timers), rose quartz lamps, redstone links, controller rails, analog levers, smart observers,
//! threshold switches and cuckoo clocks; and what comparators read of Create's blocks.
//!
//! Each block answers the game's signal questions from its state (`parts`: `block_signal`,
//! `is_signal_source`, a minecart passing a controller rail); the latches' and diodes' scheduled
//! ticks are taken from the repeater they are built on, and run in the level tick here
//! (`ticks`), as do lamps, links, rails and cuckoo clocks; levers, observers and switches look
//! after the solve (`detectors`); comparators last (`analog`, the `Comparators` subsystem).
//! What clients draw (link frequencies, lever handles and indicators, brass diodes' progress
//! indicators, cuckoo hands, doors and figures) is in `client`, sent on the redstone channels or
//! in the block entity's data (`ticks::DIODE_SHOWN`); clocks and levers ease on the client's
//! block entity ticks, and clocks tick there.
//!
//! A link's frequency is its item's id, with its colour when dyed (`frequency_key`: Create's
//! `Frequency` compares the item and `DYED_COLOR`). Links join factory gauges' panels as
//! display links do (`stock::gauge`): a receiving link pauses them, a transmitting one sends 15
//! while one is satisfied.
//!
//! A smart observer's or threshold switch's filter is read, set by hand, saved and tested by the
//! items port's filter helpers (`items::filters`); the filters of
//! broken detectors (`detectors.dropped`) are dropped there too. The threshold switch's screen
//! is `switch_screen`; the clipboard's copying and pasting of these blocks' settings is
//! `clipboard_values`.
//!
//! Left out, the API having no way to do it:
//! - A redstone link slot's item is drawn as a block item (`isGui3d`) when the item places a
//!   block: the API does not say whether an item model is 3D (`ItemRenderer` `isGui3d`).
//!
//! A rope pulley (`ThresholdSwitchObservable`) and a belt segment (its item handler) are read by
//! threshold switches as Create's are (`detectors::switch_seen`). The switch screen's link to
//! Ponder is left out with Ponder, which the port does not have.

mod analog;
mod carried;
mod client;
pub(crate) mod clipboard_values;
pub(crate) mod detectors;
mod parts;
pub(crate) mod pose;
mod saved;
mod switch_screen;
mod ticks;

pub use analog::{Comparators, analog_touched};
pub(crate) use detectors::{item_id, observed, slot_stack};
pub(crate) use parts::with_properties;
pub(crate) use ticks::show_diode;

use super::server::ServerLevel;
use super::subsystem::Subsystem;
use super::world::{try_with_world, with_world};
use create_engine::Block;
use create_engine::kinetics::Dir;
use create_engine::parts::StateInfo;
use mod_api::{
    BlockState, ClientHost, Compound, Direction, Flow, HostBlockStates, PlayerRef,
    RegisterCommandsEvent, Registrar, RegistrationContext, RunModCommandEvent, ScreenClosingEvent,
    ScreenMouseButtonPressedPreEvent, ScreenMouseScrolledPreEvent, ScreenRenderPostEvent,
    ServerHost,
};

/// `FactoryPanelSupportBehaviour.notifyLink` of a redstone link (`updateTransmittedSignal`): a
/// transmitter at `pos` sends again in the level's tick, with its panels' power.
pub(crate) fn notify_link(dim: usize, pos: [i32; 3]) {
    with_world(|w| w.dims[dim].link_checks.push(pos));
}

/// The facing of the redstone link at `pos`, if one is there.
pub(crate) fn link_facing(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Dir> {
    block_at(lv, pos)
        .and_then(|(b, i)| ticks::link_of(b, &i))
        .map(|l| l.0)
}

/// `FactoryPanelSupportBehaviour.shouldPanelBePowered` of the link at `pos`: a receiving redstone
/// link (`isListening`) that receives a signal; None for no redstone link (a display link's is
/// never powered).
pub(crate) fn link_receiving(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<bool> {
    let (_, receiver, _) = block_at(lv, pos).and_then(|(b, i)| ticks::link_of(b, &i))?;
    let dim = lv.dim;
    Some(receiver && with_world(|w| w.dims[dim].links.received(pos)).unwrap_or(0) > 0)
}

/// `LinkHandler.onBlockActivated` on the client: a click with an item on a redstone link's
/// frequency slot is the link's (the server tunes it), so the item is not used as well: armour
/// is not put on, food not eaten (the click is taken, as Create cancels it on both sides).
fn link_use_predicted(host: &dyn ClientHost, event: &mod_api::RightClickBlockEvent<'_>) -> bool {
    let click = event.click;
    let Some(stack) = click.stack else {
        return false;
    };
    let id = stack.item().id(host);
    if matches!(id.as_str(), "create:wrench" | "create:linked_controller") {
        return false;
    }
    let sneaking = host
        .local_player()
        .is_some_and(|p| p.entity().is_shift_key_down(host) == Some(true));
    if sneaking {
        return false;
    }
    let Some(level) = host.level(host.dimension()) else {
        return false;
    };
    let Some(id) = level
        .block_state(host, click.pos)
        .and_then(super::states::id)
    else {
        return false;
    };
    let Some(facing) = super::client_table(host, |t| {
        t.get(id)
            .filter(|(b, _)| *b == Block::RedstoneLink)
            .and_then(|(_, i)| i.facing)
    })
    .flatten() else {
        return false;
    };
    let local = [0, 1, 2].map(|i| click.hit.point[i] - f64::from(click.pos[i]));
    create_engine::link::hit_slot(facing, local).is_some()
}

/// The component a dyed item's colour is in (`DataComponents.DYED_COLOR`).
const DYED_COLOR: &str = "minecraft:dyed_color";

/// `RedstoneLinkNetworkHandler.Frequency.of(stack)` as a link's network key: the item, and
/// its colour when dyed (`DYED_COLOR`; Create's `equals` compares nothing else), as
/// `logistics::items::key_of` names a stack with components ("" for none).
pub(crate) fn frequency_key(
    host: &dyn mod_api::Host,
    stack: Option<&mod_api::ItemStack>,
) -> String {
    let Some(stack) = stack else {
        return String::new();
    };
    let id = stack.item().id(host);
    let dyed = stack
        .components()
        .and_then(|c| c.as_nbt().get(&DYED_COLOR.into()).cloned());
    let Some(colour) = dyed else {
        return id;
    };
    let mut plain = mod_api::ItemStack::of(stack.item());
    let patch = Compound::from([(DYED_COLOR.into(), colour)]);
    if plain.set_components(patch).is_err() {
        return id;
    }
    super::logistics::items::key_of(host, &plain)
}

/// The stack a frequency key names, to draw or copy (`Frequency.getStack`).
pub(crate) fn frequency_stack(host: &dyn mod_api::Host, key: &str) -> Option<mod_api::ItemStack> {
    if key.is_empty() {
        return None;
    }
    super::logistics::items::stack_of(host, key, 1)
}

pub struct Redstone;

/// Smart observers and threshold switches looking at their targets:
/// late in the level tick, after the machines, belts and arms moved items and before `fluids`'
/// `Tanks` hands on the tick's tank changes, which they read.
pub struct Detectors;

impl Subsystem for Detectors {
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        detectors::tick_detectors(lv);
    }
}

impl Subsystem for Redstone {
    /// A brass diode's count, for its indicator (`ticks::DIODE_SHOWN`).
    fn synced_keys(&self) -> &'static [&'static str] {
        &[ticks::DIODE_SHOWN]
    }

    /// A cuckoo clock's hands and doors, a brass diode's indicator and an analog lever's handle
    /// are drawn each frame (`client`).
    fn animated(
        &self,
        block: create_engine::Block,
        _pos: [i32; 3],
        _entity: &mod_api::ModBlockEntity,
    ) -> bool {
        matches!(
            block,
            create_engine::Block::CuckooClock
                | create_engine::Block::MysteriousCuckooClock
                | create_engine::Block::PulseRepeater
                | create_engine::Block::PulseExtender
                | create_engine::Block::PulseTimer
                | create_engine::Block::AnalogLever
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

    /// `AnalogLeverBlockEntity.addToGoggleTooltip`: its signal strength.
    fn goggle_tooltip(
        &self,
        _host: &dyn mod_api::ClientHost,
        pos: [i32; 3],
        block: create_engine::Block,
        lines: &mut Vec<Vec<(String, u32)>>,
    ) {
        if block == create_engine::Block::AnalogLever
            && let Some(state) = client::lever_state(pos)
        {
            lines.push(create_engine::gauge::goggles(
                0,
                vec![(format!("Analog Strength: {state}/15"), 0xFFFFFF)],
            ));
        }
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

    fn carry_out(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        block: create_engine::Block,
        out: &mut Compound,
    ) {
        carried::carry_out(lv, pos, block, out);
    }

    fn carry_in(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        block: create_engine::Block,
        data: &Compound,
        _turn: create_engine::contraption::Transform,
    ) {
        carried::carry_in(lv, pos, block, data);
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
            || switch_screen::server_message(host, channel, data)
    }

    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        switch_screen::client_message(host, player, channel, data)
            || clipboard_values::client_message(host, player, channel, data)
    }

    fn right_click_block(
        &self,
        host: mod_api::HostSide,
        event: &mut mod_api::RightClickBlockEvent<'_>,
    ) -> Flow {
        if let mod_api::HostSide::Client(h) = &host
            && link_use_predicted(&**h, event)
        {
            *event.cancellation_result = mod_api::InteractionResult::Success;
            return Flow::Cancel;
        }
        clipboard_values::right_click_block(host, event)
    }

    fn interaction_key(
        &self,
        host: &mut dyn ClientHost,
        event: &mut mod_api::InteractionKeyMappingTriggeredEvent,
    ) -> Flow {
        clipboard_values::interaction_key(host, event)
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        client::tick(host);
        switch_screen::tick(host);
    }

    fn client_reset(&self) {
        client::reset();
        switch_screen::reset();
    }

    fn input_handled(&self, host: &mut dyn ClientHost) {
        switch_screen::flush(host);
    }

    fn screen_render_post(&self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        switch_screen::screen_render_post(host, event);
    }

    fn screen_mouse_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        switch_screen::screen_mouse_pressed(host, event)
    }

    fn screen_mouse_scrolled(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseScrolledPreEvent<'_>,
    ) -> Flow {
        switch_screen::screen_mouse_scrolled(host, event)
    }

    fn screen_closing(&self, host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
        switch_screen::screen_closing(host, event);
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

/// The engine block at `pos` of this client's level, for Create's blocks.
pub(crate) fn client_block_of(host: &dyn ClientHost, pos: [i32; 3]) -> Option<Block> {
    let level = host.level(host.dimension())?;
    let id = super::states::id(level.block_state(host, pos)?)?;
    super::client_table(host, |t| t.get(id).map(|(b, _)| b)).flatten()
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
