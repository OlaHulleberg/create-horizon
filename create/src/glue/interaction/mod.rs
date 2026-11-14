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
//! Left out (no API for it):
//! - The goggles' tooltip is drawn as a plain box of fills and text: Create draws vanilla's
//!   tooltip background (`RemovedGuiUtils.drawHoveringText`); `DrawContext::gui_tooltip` takes
//!   one colour per line where Create's lines change colour along the way.
//! - `ValueSettingsScreen`'s sprites (`AllGuiTextures.VALUE_SETTINGS_*`) are plain fills, as in
//!   the legacy glue: the GUI sprites are the player's jar's, not the mod's assets.

mod board;
mod commands;
mod crank;
mod hud;
/// The value channels and the value box rules, for the subsystems whose blocks have one.
pub(crate) mod values;

use super::subsystem::Subsystem;
use mod_api::{
    ClientHost, Flow, HostSide, MouseScrollingEvent, PlayerRef, RegisterCommandsEvent, Registrar,
    RegistrationContext, RenderGuiPostEvent, RightClickBlockEvent, RunModCommandEvent, ServerHost,
};
use values::VALUE;

pub struct Interaction;

impl Subsystem for Interaction {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        crank::register(r);
    }

    fn client_reset(&self) {
        board::reset();
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
            values::SCROLL => values::scroll_from_client(host, player, data),
            values::VALUE_SETTINGS => values::value_settings_from_client(host, player, data),
            _ => return false,
        }
        true
    }

    fn server_message(&self, _host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        if channel != VALUE {
            return false;
        }
        board::values_from_server(data);
        true
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        board::tick(host);
    }

    fn mouse_scrolling(&self, host: &mut dyn ClientHost, event: &mut MouseScrollingEvent) -> Flow {
        board::wheel(host, event.accumulated[1])
    }

    /// The board when one is open, else the value tip and the goggles' tooltip.
    fn render_gui_post(&self, host: &dyn ClientHost, event: &mut RenderGuiPostEvent<'_>) {
        if board::draw(host, event.draw) {
            return;
        }
        hud::value_tip(host, event.draw);
        hud::goggle_overlay(host, event.draw);
    }

    /// The highlighted value box's outline.
    fn render_level_stage(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::RenderLevelStageEvent<'_>,
    ) {
        hud::value_box_outline(host, event);
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
