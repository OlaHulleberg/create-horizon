//! Create 6.0.11 for Horizon, as a native mod on the fork's mod API.
//!
//! Create's rules are the `create-engine` crate (`src/engine`): plain Rust with no game types,
//! unit-tested on their own. `glue` drives them through the mod API: each hook below is written
//! once and handed to every subsystem (`glue::SUBSYSTEMS`); per-block moments are Parts.

mod glue;

use glue::server::ServerLevel;
use glue::{SUBSYSTEMS, persist, states, world};
use mod_api::{
    BlockChangedEvent, BlockPart, ChunkLoadEvent, ChunkUnloadEvent, ClientHost,
    ClientLevelUnloadEvent, ClientTickPostEvent, Hooks, HostSide, LevelHost, LevelLoadEvent,
    LevelTickPostEvent, LevelTicksHost, NeighborChangedEvent, PlayerLoggedInEvent, Registrar,
    RegistrationContext, ServerHost, WorldLifecycleEvent, WorldPhase,
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

    fn client_level_unload(
        &self,
        host: &mut dyn ClientHost,
        _event: &mut ClientLevelUnloadEvent<'_>,
    ) {
        glue::client_reset(host);
    }
}

/// Every Create block's part: the moments that belong to one block.
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
