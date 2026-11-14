//! Create 6.0.11 for Horizon, as a native mod on the fork's mod API.
//!
//! Create's rules are the `create-engine` crate (`src/engine`): plain Rust with no game types,
//! unit-tested on their own. `glue` drives them through the mod API: each hook below is written
//! once and handed to every subsystem (`glue::SUBSYSTEMS`); per-block moments are Parts.

mod glue;

use glue::server::ServerLevel;
use glue::{SUBSYSTEMS, persist, states, world};
use mod_api::{
    AttackEntityEvent, BlockChangedEvent, BlockPart, BreakBlockEvent, ChunkLoadEvent,
    ChunkUnloadEvent, ClientHost, ClientLevelUnloadEvent, ClientTickPostEvent, EntityInteractEvent,
    EntityJoinLevelEvent, EntityMountEvent, EntityPlaceEvent, Flow, Hooks, HostSide, InputKeyEvent,
    InteractionResult, KeyMappingPreEvent, LeftClickBlockEvent, LevelHost, LevelLoadEvent,
    LevelSaveEvent, LevelTickPostEvent, LevelTicksHost, MouseScrollingEvent, NeighborChangedEvent,
    PlayerLoggedInEvent, PlayerTickPostEvent, RegisterCommandsEvent, Registrar,
    RegistrationContext, RenderGuiPostEvent, RightClickBlockEvent, RightClickItemEvent,
    RunModCommandEvent, ServerHost, WorldLifecycleEvent, WorldPhase,
};

#[derive(Default)]
struct Create;

#[mod_api::horizon_mod]
impl Hooks for Create {
    fn register(&self, r: &mut dyn Registrar, cx: &RegistrationContext) {
        states::register(r);
        for &block in create_engine::Block::ALL.iter() {
            r.block_part(states::key(block), CreateBlock);
        }
        glue::net::register(r, SUBSYSTEMS);
        for s in SUBSYSTEMS {
            s.register(r, cx);
        }
    }

    fn world_lifecycle(&self, _host: &mut dyn ServerHost, event: &mut WorldLifecycleEvent<'_>) {
        match event.phase {
            WorldPhase::WorldCloseFinal
            | WorldPhase::WorldOpenFailed
            | WorldPhase::WorldCloseFailed => {
                for s in SUBSYSTEMS {
                    s.world_closed();
                }
                world::close();
                persist::clear();
                glue::net::clear();
                glue::forget_columns();
            }
            _ => {}
        }
    }

    /// A level opened, before any of its chunks: the first opens Create's world.
    fn level_load(&self, host: &mut dyn ServerHost, event: &mut LevelLoadEvent<'_>) {
        if !world::is_open() && world::open(&*host) {
            for s in SUBSYSTEMS {
                s.world_opened(host);
            }
        }
        world::level_index(event.dimension);
    }

    fn chunk_load(&self, host: &mut dyn ServerHost, event: &mut ChunkLoadEvent<'_>) {
        glue::ensure_column(host, event.dimension, event.chunk);
    }

    fn chunk_unload(&self, _host: &mut dyn ServerHost, event: &mut ChunkUnloadEvent<'_>) {
        let Some(dim) = world::level_index(event.dimension) else {
            return;
        };
        glue::forget_column(dim, event.chunk);
        for s in SUBSYSTEMS {
            s.chunk_unload(dim, event.chunk);
        }
    }

    /// Hot: every committed block change on the server. Two lookups, then return, for any
    /// change that is not Create's.
    fn block_changed(&self, _level: &dyn LevelHost, event: &mut BlockChangedEvent) {
        let (old, new) = (states::id_or_none(event.old), states::id_or_none(event.new));
        if old == 0 && new == 0 {
            return;
        }
        let Some(dim) = world::level_index(event.dimension.key().id()) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        for s in SUBSYSTEMS {
            s.block_changed(dim, pos, old, new);
        }
    }

    fn level_tick_post(&self, host: HostSide, event: &mut LevelTickPostEvent) {
        let HostSide::Server(host) = host else {
            return;
        };
        let Some(dimension) = event.level.dimension().id(&*host) else {
            return;
        };
        glue::net::flush(host);
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        for s in SUBSYSTEMS {
            s.level_tick(&mut lv);
        }
        persist::flush(&mut lv, SUBSYSTEMS);
    }

    fn player_logged_in(&self, host: &mut dyn ServerHost, event: &mut PlayerLoggedInEvent<'_>) {
        if !world::is_open() {
            return;
        }
        let Some(player) = host.player(event.player) else {
            return;
        };
        for s in SUBSYSTEMS {
            s.player_joined(host, player);
        }
    }

    fn client_tick_post(&self, host: &mut dyn ClientHost, _event: &mut ClientTickPostEvent) {
        for s in SUBSYSTEMS {
            s.client_tick(host);
        }
    }

    fn entity_place(&self, host: &mut dyn ServerHost, event: &mut EntityPlaceEvent) -> Flow {
        until_cancel(|s| s.entity_place(host, event))
    }

    fn right_click_block(&self, mut host: HostSide, event: &mut RightClickBlockEvent<'_>) -> Flow {
        until_cancel(|s| s.right_click_block(reborrow(&mut host), event))
    }

    fn right_click_item(&self, mut host: HostSide, event: &mut RightClickItemEvent<'_>) -> Flow {
        until_cancel(|s| s.right_click_item(reborrow(&mut host), event))
    }

    fn left_click_block(&self, host: &mut dyn ServerHost, event: &mut LeftClickBlockEvent) -> Flow {
        until_cancel(|s| s.left_click_block(host, event))
    }

    fn break_block(&self, host: &mut dyn ServerHost, event: &mut BreakBlockEvent<'_>) -> Flow {
        until_cancel(|s| s.break_block(host, event))
    }

    fn entity_interact(
        &self,
        mut host: HostSide,
        event: &mut EntityInteractEvent<'_>,
    ) -> Option<InteractionResult> {
        SUBSYSTEMS
            .iter()
            .find_map(|s| s.entity_interact(reborrow(&mut host), event))
    }

    fn attack_entity(&self, host: &mut dyn ServerHost, event: &mut AttackEntityEvent<'_>) -> Flow {
        until_cancel(|s| s.attack_entity(host, event))
    }

    fn entity_mount(&self, host: &mut dyn ServerHost, event: &mut EntityMountEvent<'_>) -> Flow {
        until_cancel(|s| s.entity_mount(host, event))
    }

    fn entity_join_level(
        &self,
        host: &mut dyn ServerHost,
        event: &mut EntityJoinLevelEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.entity_join_level(host, event))
    }

    fn key_mapping_pre(
        &self,
        host: &mut dyn ClientHost,
        event: &mut KeyMappingPreEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.key_mapping_pre(host, event))
    }

    fn player_tick_post(&self, mut host: HostSide, event: &mut PlayerTickPostEvent) {
        for s in SUBSYSTEMS {
            s.player_tick(reborrow(&mut host), event);
        }
    }

    fn register_commands(&self, host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        for s in SUBSYSTEMS {
            s.register_commands(host, event);
        }
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.run_mod_command(host, event))
    }

    fn level_save(&self, host: &mut dyn ServerHost, event: &mut LevelSaveEvent<'_>) {
        for s in SUBSYSTEMS {
            s.level_save(host, event);
        }
    }

    fn input_key(&self, host: &mut dyn ClientHost, event: &mut InputKeyEvent<'_>) {
        for s in SUBSYSTEMS {
            s.input_key(host, event);
        }
    }

    fn mouse_scrolling(&self, host: &mut dyn ClientHost, event: &mut MouseScrollingEvent) -> Flow {
        until_cancel(|s| s.mouse_scrolling(host, event))
    }

    fn render_gui_post(&self, host: &dyn ClientHost, event: &mut RenderGuiPostEvent<'_>) {
        for s in SUBSYSTEMS {
            s.render_gui_post(host, event);
        }
    }

    fn client_level_unload(
        &self,
        host: &mut dyn ClientHost,
        _event: &mut ClientLevelUnloadEvent<'_>,
    ) {
        glue::client_reset(host);
    }
}

/// Asks each subsystem in order until one cancels.
fn until_cancel(mut f: impl FnMut(&dyn glue::subsystem::Subsystem) -> Flow) -> Flow {
    for s in SUBSYSTEMS {
        if let Flow::Cancel = f(*s) {
            return Flow::Cancel;
        }
    }
    Flow::Continue
}

/// The same host again, for the next subsystem.
fn reborrow<'a>(host: &'a mut HostSide<'_>) -> HostSide<'a> {
    match host {
        HostSide::Server(h) => HostSide::Server(&mut **h),
        HostSide::Client(h) => HostSide::Client(&mut **h),
    }
}

/// Every Create block's part: the engine's per-block moments. Subsystems attach their own
/// parts to their own blocks for the rest.
struct CreateBlock;

#[mod_api::horizon_part]
impl BlockPart for CreateBlock {
    /// A neighbour of a Create block changed (`neighborChanged`). Runs inside the write: the
    /// subsystems note it and act in the level tick.
    fn neighbor_changed(&self, _level: &mut dyn LevelTicksHost, event: &mut NeighborChangedEvent) {
        let Some(state) = states::id(event.state) else {
            return;
        };
        let Some(dim) = world::level_index(event.dimension.key().id()) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        for s in SUBSYSTEMS {
            s.neighbor_changed(dim, pos, state);
        }
    }
}
