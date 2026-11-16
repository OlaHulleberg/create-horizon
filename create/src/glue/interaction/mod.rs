//! A player's hands on Create's blocks: value boxes and Create 6's value settings boards
//! (`ValueSettingsInputHandler`, `ValueSettingsClient`, `ValueSettingsScreen`), scrolling a
//! value with the mouse wheel (`ScrollValueBehaviour`), hand cranks and valve handles turned by
//! hand, the HUD tips over the hotbar and the goggles' tooltip (`GoggleOverlayRenderer`), and
//! Create's `/create_debug speed` command.
//!
//! The server owns every value: a client's scroll, board pick or short click goes to it on
//! `create:scroll` / `create:value_settings` (checked there: the player's own level, the block
//! within reach, not in adventure or spectator), and the setting comes back on `create:value`,
//! which the client's value box and board show.
//!
//! The goggles' tooltip is drawn as `RemovedGuiUtils.drawHoveringText` draws it (its gradient
//! box in the overlay's colours, faded in, each line's parts in their own colours), and the
//! board with `ValueSettingsScreen`'s sprites (`create:gui/value_settings`, its icons from
//! `create:gui/icons`).

mod board;
mod commands;
mod crank;
mod ghost;
mod ghost_models;
mod hud;
mod placement;
/// The value channels and the value box rules, for the subsystems whose blocks have one.
pub(crate) mod value_text;
pub(crate) mod values;

use super::subsystem::Subsystem;
use mod_api::{
    ClientHost, Flow, HostSide, MouseScrollingEvent, PlayerRef, RegisterCommandsEvent, Registrar,
    RegistrationContext, RenderGuiPostEvent, RightClickBlockEvent, RunModCommandEvent, ServerHost,
};
use values::VALUE;

pub struct Interaction;

/// A placement helper's target and the properties its transform sets on the held block's
/// default state (`PlacementOffset.withTransform`), for the ghost.
pub(crate) type HelperTarget = ([i32; 3], Vec<(&'static str, String)>);

/// The placement ghosts' world pipelines, one a sprite (`ghost.rs`).
pub(crate) fn register_pipelines(event: &mut mod_api::RegisterRenderPipelinesEvent<'_>) {
    ghost::register(event);
}

impl Subsystem for Interaction {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        crank::register(r);
    }

    fn client_reset(&self) {
        board::reset();
        placement::reset();
    }

    fn world_closed(&self) {
        placement::world_closed();
    }

    /// The placement helpers' targets, for their indicator.
    fn player_tick(&self, host: HostSide, event: &mut mod_api::PlayerTickPostEvent) {
        if let HostSide::Server(host) = host {
            placement::player_tick(host, event.player);
        }
    }

    /// `ValueSettingsInputHandler.onBlockActivated` on both sides, and the client's prediction
    /// of a hand crank's or valve handle's use (`HandCrankBlock.useItemOn` answers SUCCESS on
    /// the client too).
    fn right_click_block(&self, host: HostSide, event: &mut RightClickBlockEvent<'_>) -> Flow {
        match host {
            HostSide::Server(host) => values::right_click(host, event),
            HostSide::Client(host) => board::right_click(host, event),
        }
    }

    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        match channel {
            values::VALUE_SETTINGS => values::value_settings_from_client(host, player, data),
            _ => return false,
        }
        true
    }

    fn server_message(&self, _host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        match channel {
            VALUE => board::values_from_server(data),
            placement::CHANNEL => placement::from_server(data),
            _ => return false,
        }
        true
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        board::tick(host);
        value_text::tick(host);
        placement::tick();
    }

    /// The block looked at is drawn each frame while it shows its value box's number.
    fn animated(
        &self,
        _block: create_engine::Block,
        pos: [i32; 3],
        _entity: &mod_api::ModBlockEntity,
    ) -> bool {
        value_text::animated(pos)
    }

    fn camera_pose(&self, _host: &dyn ClientHost, event: &mut mod_api::CameraPoseEvent<'_>) {
        board::camera_pose(event);
    }

    /// The use key is taken while a value box's press or board is held.
    fn interaction_key(
        &self,
        _host: &mut dyn ClientHost,
        event: &mut mod_api::InteractionKeyMappingTriggeredEvent,
    ) -> Flow {
        board::interaction_key(event)
    }

    fn mouse_scrolling(&self, host: &mut dyn ClientHost, event: &mut MouseScrollingEvent) -> Flow {
        board::wheel(host, event.accumulated[1])
    }

    /// The board when one is open, else the value tip and the goggles' tooltip.
    fn render_gui_post(&self, host: &dyn ClientHost, event: &mut RenderGuiPostEvent<'_>) {
        if board::draw(host, event.draw) {
            return;
        }
        // `PlacementClient.onRenderCrosshairOverlay`, with the crosshair.
        placement::draw(host, event.draw);
        hud::value_tip(host, event.draw);
        hud::goggle_overlay(host, event.draw);
    }

    /// The highlighted value box's outline and the placement helper's ghost.
    fn render_level_stage(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::RenderLevelStageEvent<'_>,
    ) {
        hud::value_box_outline(host, event);
        placement::draw_ghost(host, event);
    }

    fn register_commands(&self, _host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        commands::register(event.commands);
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        commands::run(host, event)
    }
}
