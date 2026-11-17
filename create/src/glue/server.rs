//! One server level as Create's glue sees it: the host, the level's handle and its engine index,
//! with the reads and writes the glue makes in engine terms (positions as `[i32; 3]`, states as
//! engine ids). Every subsystem works through this rather than the raw API, so the conversions
//! live in one place. Subsystems add their own methods in their own modules
//! (`impl ServerLevel<'_> { .. }`).

use super::states;
use mod_api::{
    BlockState, BlockUpdateFlags, Compound, Direction, HostBlockStates, LevelRef, NeighbourUpdate,
    PlayerRef, ServerHost, Tag,
};

/// `minecraft:block`'s particle key, for block break particles (set at registration by the
/// water wheels, the first to draw them).
pub static BLOCK_PARTICLE: std::sync::OnceLock<mod_api::ParticleKey> = std::sync::OnceLock::new();

pub struct ServerLevel<'a> {
    pub host: &'a mut dyn ServerHost,
    pub level: LevelRef,
    /// The engine's index of this level.
    pub dim: usize,
    /// `minecraft:overworld`.
    pub dimension: &'a str,
}

/// Vanilla's `Block.UPDATE_CLIENTS`.
pub const UPDATE_CLIENTS: BlockUpdateFlags = BlockUpdateFlags::CLIENTS;

/// Vanilla's `Block.UPDATE_ALL`: neighbours and clients.
pub fn update_all() -> BlockUpdateFlags {
    BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS
}

impl<'a> ServerLevel<'a> {
    /// `levelEvent(LevelEvent.PARTICLES_DESTROY_BLOCK, pos, Block.getId(state))`, rebuilt from
    /// parts (the API takes no state id for it): `state`'s break particles over the block (64,
    /// in its box) and its sound type's break sound at `(volume + 1) / 2` and `pitch * 0.8`.
    pub fn destroy_effect(&mut self, pos: [i32; 3], state: BlockState) {
        let centre = pos.map(|v| f64::from(v) + 0.5);
        if let Some(&key) = BLOCK_PARTICLE.get() {
            let options = mod_api::ParticleOptions::simple(key);
            let _ = self.level.send_particles(
                &mut *self.host,
                &options,
                Some(state),
                centre,
                [0.25; 3],
                0.0,
                64,
            );
        }
        if let Some(sound) = self.host.block_states().sound(state) {
            let _ = self.level.play_sound(
                &mut *self.host,
                &sound.break_sound,
                centre,
                mod_api::SoundSource::Blocks,
                (sound.volume + 1.0) / 2.0,
                sound.pitch * 0.8,
            );
        }
    }

    /// The level `dimension` names, when it is open and Create's world is.
    pub fn new(host: &'a mut dyn ServerHost, dimension: &'a str) -> Option<ServerLevel<'a>> {
        let dim = super::world::level_index(dimension)?;
        let level = host.level(dimension)?;
        Some(ServerLevel {
            host,
            level,
            dim,
            dimension,
        })
    }

    /// The state at `pos`, or `None` where the level has none loaded.
    pub fn block_state(&self, pos: [i32; 3]) -> Option<BlockState> {
        self.level.block_state(&*self.host, pos)
    }

    /// The engine id at `pos`: Create's state, or 0 for anything else (or nothing loaded).
    pub fn block(&self, pos: [i32; 3]) -> u32 {
        self.block_state(pos).map_or(0, states::id_or_none)
    }

    /// Whether `pos` is in a loaded chunk.
    pub fn is_loaded(&self, pos: [i32; 3]) -> bool {
        self.level.is_loaded(&*self.host, pos)
    }

    /// Sets the state at `pos` (`Level.setBlock`). True when the block changed.
    pub fn set_state(&mut self, pos: [i32; 3], state: BlockState, flags: BlockUpdateFlags) -> bool {
        self.level
            .set_block(&mut *self.host, pos, state, flags)
            .unwrap_or(false)
    }

    /// Sets one property of the state at `pos` (`state.setValue(..)` then `setBlock`). False when
    /// the block has no such property or value, or nothing changed.
    pub fn set_property(
        &mut self,
        pos: [i32; 3],
        key: &str,
        value: &str,
        flags: BlockUpdateFlags,
    ) -> bool {
        let Some(state) = self.block_state(pos) else {
            return false;
        };
        let Ok(next) = state.with(&self.host.block_states(), key, value) else {
            return false;
        };
        next != state && self.set_state(pos, next, flags)
    }

    /// `Level.hasNeighborSignal`.
    pub fn has_neighbor_signal(&self, pos: [i32; 3]) -> bool {
        self.level.has_neighbor_signal(&*self.host, pos)
    }

    /// `Level.getBestNeighborSignal`.
    pub fn best_neighbor_signal(&self, pos: [i32; 3]) -> i32 {
        self.level
            .best_neighbor_signal(&*self.host, pos)
            .unwrap_or(0)
    }

    /// `Level.getSignal(pos, direction)`.
    pub fn signal(&self, pos: [i32; 3], direction: Direction) -> i32 {
        self.level.signal(&*self.host, pos, direction).unwrap_or(0)
    }

    /// `Level.destroyBlock(pos, drops)`. A toolbox's drop keeps its contents.
    pub fn destroy_block(&mut self, pos: [i32; 3], drops: bool) -> bool {
        if drops {
            crate::glue::processing::toolbelt::before_removal(self, pos);
        }
        matches!(
            self.level
                .destroy_block(&mut *self.host, pos, drops, None, false),
            Ok(Some(_))
        )
    }

    /// One compound a block entity's `write` owns, written as a whole under `key`: the entity
    /// rewrites its fields, so one it no longer writes must go. `merge_data` merges nested
    /// compounds recursively (as `/data merge`) and would keep it. False when no block entity
    /// is at `pos` or its data did not change.
    pub fn write_entity_field(&mut self, pos: [i32; 3], key: &str, value: Compound) -> bool {
        let Some(entity) = self.host.block_entity(self.level, pos) else {
            return false;
        };
        let Some(mut data) = entity.data(&*self.host) else {
            return false;
        };
        data.insert(key.into(), Tag::Compound(value));
        entity.replace_data(&mut *self.host, data) == Ok(true)
    }

    /// The level's game time.
    pub fn game_time(&self) -> i64 {
        self.level.game_time(&*self.host).unwrap_or(0)
    }

    /// The players in this level.
    pub fn players(&self) -> Vec<PlayerRef> {
        self.level.online_players(&*self.host).unwrap_or_default()
    }

    /// Runs a command as the server, in this level (`execute in <dimension> run ..`).
    pub fn run_command(&mut self, line: &str) -> bool {
        let line = format!("execute in {} run {line}", self.dimension);
        self.host.commands().run_command(&line)
    }

    /// The block's id at `p` (`minecraft:fire`).
    pub fn block_name(&self, p: [i32; 3]) -> Option<String> {
        let state = self.block_state(p)?;
        let text = self.host.block_state_string(state)?;
        Some(states::split(&text).0.to_owned())
    }

    /// Sets boolean properties of the block at `pos` (`setBlock` with `flags`); whether its state
    /// changed.
    pub fn set_flags(
        &mut self,
        pos: [i32; 3],
        props: &[(&str, bool)],
        flags: BlockUpdateFlags,
    ) -> bool {
        let Some(state) = self.block_state(pos) else {
            return false;
        };
        let mut next = state;
        {
            let facts = self.host.block_states();
            for (k, v) in props {
                if let Ok(s) = next.with(&facts, k, if *v { "true" } else { "false" }) {
                    next = s;
                }
            }
        }
        next != state && self.set_state(pos, next, flags)
    }

    /// `Level.updateNeighborsAt`: the six neighbours of `pos` hear of it (a signal changed).
    pub fn update_signal_neighbors(&mut self, pos: [i32; 3]) {
        if let Some(state) = self.block_state(pos) {
            let _ = self.level.update_neighbors(
                &mut *self.host,
                pos,
                state,
                NeighbourUpdate::Neighbours,
            );
        }
    }

    /// `Level.updateNeighbourForOutputSignal`: comparators beside `pos` read it again.
    pub fn update_output_signal(&mut self, pos: [i32; 3]) {
        if let Some(state) = self.block_state(pos) {
            let _ = self.level.update_neighbors(
                &mut *self.host,
                pos,
                state,
                NeighbourUpdate::OutputSignal,
            );
        }
    }
}
