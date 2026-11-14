//! Create's block states as the engine numbers them.
//!
//! The mod API names a state by its block and a process-local ordinal (`BlockState`), not by the
//! game's numeric id. The engine wants one contiguous run of `u32` ids so its hot tests stay one
//! compare (`create_engine::states`). This table gives each of Create's blocks a base in that run,
//! in `Block` order, and each state its block's base plus its ordinal; it maps both ways.
//!
//! Block keys are requested once at registration; the states and their properties are read
//! when the first world opens, from the host's block-state facts.

use create_engine::Block;
use create_engine::world::StateTable;
use mod_api::{BlockKey, BlockState, Host, HostBlockStates, Registrar};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{OnceLock, RwLock};

/// Where Create's ids start. Any value works; a large one keeps them apart from small numbers
/// the engine's tests use for vanilla blocks.
pub const FIRST: u32 = 1 << 20;

/// The key of each of Create's blocks, by `Block as usize`, requested in `register`.
static KEYS: OnceLock<Vec<BlockKey>> = OnceLock::new();

/// Requests the key of every block the engine knows (`create:<name>`).
pub fn register(r: &mut dyn Registrar) {
    let keys = Block::NAMES
        .iter()
        .map(|name| r.block_key(&format!("create:{name}")))
        .collect();
    // A second registration (a second game in the process) asks for the same keys.
    let _ = KEYS.set(keys);
}

/// The key of `block`. Panics before `register`, which runs before any world.
pub fn key(block: Block) -> BlockKey {
    KEYS.get()
        .expect("Create's block keys are requested at registration")[block as usize]
}

/// A hasher for `BlockKey`, whose hash is one `u32` slot: the slot itself is the hash.
#[derive(Default)]
pub struct SlotHasher(u64);

impl Hasher for SlotHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(b);
        }
    }
    fn write_u32(&mut self, n: u32) {
        self.0 = u64::from(n);
    }
}

type SlotMap<V> = HashMap<BlockKey, V, BuildHasherDefault<SlotHasher>>;

/// Both directions of the numbering, once a world has opened.
pub struct States {
    /// Each block's first id and its state count.
    base: SlotMap<(u32, u32)>,
    /// The API state of each id, `FIRST..`.
    by_id: Vec<BlockState>,
}

static STATES: RwLock<Option<States>> = RwLock::new(None);

impl States {
    /// Reads every state of Create's blocks from the host and builds the engine's table.
    /// `None` when Create's blocks are not in this game (the content is off).
    fn read(host: &dyn Host) -> Option<(States, StateTable)> {
        let keys = KEYS.get()?;
        let facts = host.block_states();
        let mut base = SlotMap::default();
        let mut by_id = Vec::new();
        let mut names = Vec::new();
        for (i, &key) in keys.iter().enumerate() {
            let Some(range) = facts.states_of(key) else {
                continue;
            };
            let first = FIRST + by_id.len() as u32;
            base.insert(key, (first, range.len()));
            for state in range.iter() {
                by_id.push(state);
                let text = host
                    .block_state_string(state)
                    .unwrap_or_else(|| format!("create:{}", Block::NAMES[i]));
                names.push(text);
            }
        }
        if by_id.is_empty() {
            return None;
        }
        let parsed: Vec<(&str, Vec<(&str, &str)>)> = names.iter().map(|t| split(t)).collect();
        let table = StateTable::new(
            FIRST,
            parsed
                .iter()
                .map(|(name, props)| (*name, props.iter().copied())),
        );
        Some((States { base, by_id }, table))
    }

    fn id(&self, state: BlockState) -> Option<u32> {
        let &(first, len) = self.base.get(&state.block())?;
        let ordinal = u32::from(state.ordinal());
        (ordinal < len).then_some(first + ordinal)
    }
}

/// `create:shaft[axis=y,waterlogged=false]` as its name and properties.
pub fn split(text: &str) -> (&str, Vec<(&str, &str)>) {
    match text.split_once('[') {
        None => (text, Vec::new()),
        Some((name, rest)) => {
            let props = rest
                .trim_end_matches(']')
                .split(',')
                .filter_map(|p| p.split_once('='))
                .collect();
            (name, props)
        }
    }
}

/// Builds the numbering for the world being opened (on the server, or on a client joining one)
/// and returns the engine's table, or `None` when Create's blocks are absent. The ids are the
/// same for every world and both sides of this process (the states never change while it runs),
/// so the numbering is kept once built.
pub fn open(host: &dyn Host) -> Option<StateTable> {
    let (states, table) = States::read(host)?;
    create_engine::states::set_range(FIRST, states.by_id.len() as u32);
    *STATES.write().unwrap_or_else(|e| e.into_inner()) = Some(states);
    Some(table)
}

/// The engine id of `state`, or `None` for a block that is not Create's. Hot: one map lookup.
#[inline]
pub fn id(state: BlockState) -> Option<u32> {
    STATES.read().ok()?.as_ref()?.id(state)
}

/// The engine id of `state`, or 0 (no Create block) for any other: the form the engine's
/// `block_changed` and friends take.
#[inline]
pub fn id_or_none(state: BlockState) -> u32 {
    id(state).unwrap_or(0)
}

/// The API state of engine id `id`.
pub fn state(id: u32) -> Option<BlockState> {
    let i = id.checked_sub(FIRST)? as usize;
    STATES.read().ok()?.as_ref()?.by_id.get(i).copied()
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn splits_state_strings() {
        assert_eq!(split("create:shaft"), ("create:shaft", vec![]));
        assert_eq!(
            split("create:shaft[axis=y,waterlogged=false]"),
            (
                "create:shaft",
                vec![("axis", "y"), ("waterlogged", "false")]
            )
        );
    }
}
