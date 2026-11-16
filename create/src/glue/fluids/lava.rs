//! Honey and chocolate meeting lava (`AllFluids.registerFluidInteractions`): lava with honey or
//! chocolate as its neighbour, above or beside it as vanilla lava meets water, becomes
//! obsidian where it is a source, else limestone (honey) or scoria (chocolate), with the fizz
//! (`LiquidBlock.fizz`). Create's lava does it at once (`shouldSpreadLiquid` on a neighbour's
//! change); here it is found when the lava, or the honey or chocolate beside it, next ticks
//! (`fluid_tick`, which can't write) and done in the level's tick after.

use super::super::server::ServerLevel;
use super::super::world::level_index;
use super::{all, state_with};
use mod_api::{FluidKey, FluidTickEvent, LevelHost};
use std::cell::RefCell;

/// The world fluids this rule reads.
const IDS: [&str; 6] = [
    "minecraft:lava",
    "minecraft:flowing_lava",
    "create:honey",
    "create:flowing_honey",
    "create:chocolate",
    "create:flowing_chocolate",
];

thread_local! {
    /// Lava blocks to turn, by level: position and what they become.
    static TURNING: RefCell<Vec<(usize, [i32; 3], &'static str)>> = const { RefCell::new(Vec::new()) };
    /// The keys of `IDS`, looked up on the server's first level tick (a fluid tick's level
    /// can't name a key).
    static KEYS: RefCell<Vec<(FluidKey, &'static str)>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn reset() {
    TURNING.with_borrow_mut(Vec::clear);
    KEYS.with_borrow_mut(Vec::clear);
}

/// What `id` is to this rule: lava, or the block lava meeting it becomes (honey: limestone,
/// chocolate: scoria).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Lava,
    Meets(&'static str),
}

fn kind(id: &str) -> Option<Kind> {
    Some(match id {
        "minecraft:lava" | "minecraft:flowing_lava" => Kind::Lava,
        "create:honey" | "create:flowing_honey" => Kind::Meets("create:limestone"),
        "create:chocolate" | "create:flowing_chocolate" => Kind::Meets("create:scoria"),
        _ => return None,
    })
}

/// The fluid at `pos`, what it is to this rule and whether it is a source.
fn fluid_at(level: &dyn LevelHost, pos: [i32; 3]) -> Option<(Kind, bool)> {
    let f = level.fluid_state(pos)?;
    if f.is_empty() {
        return None;
    }
    let id = KEYS.with_borrow(|k| k.iter().find(|(key, _)| *key == f.fluid).map(|e| e.1))?;
    Some((kind(id)?, f.is_source()))
}

/// `FluidInteractionRegistry.canInteract` for lava at `pos`: honey or chocolate on a side lava
/// would flow from (`LiquidBlock.POSSIBLE_FLOW_DIRECTIONS`' opposites: above and the four
/// sides), in that order.
fn meets(level: &dyn LevelHost, pos: [i32; 3]) -> Option<&'static str> {
    const FROM: [[i32; 3]; 5] = [[0, 1, 0], [0, 0, -1], [0, 0, 1], [1, 0, 0], [-1, 0, 0]];
    FROM.iter().find_map(|o| {
        let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
        match fluid_at(level, p)? {
            (Kind::Meets(block), _) => Some(block),
            _ => None,
        }
    })
}

/// A world fluid's tick: lava that meets honey or chocolate (its own, or the honey's or
/// chocolate's below or beside it) is turned in the next level tick; a lava tick that turns
/// is skipped.
pub(super) fn fluid_tick(level: &dyn LevelHost, event: &mut FluidTickEvent<'_>) {
    let Some(this) = kind(event.fluid) else {
        return;
    };
    let Some(dim) = level_index(event.dimension.key().id()) else {
        return;
    };
    let pos = [event.pos.x, event.pos.y, event.pos.z];
    let lava: Vec<[i32; 3]> = match this {
        Kind::Lava => vec![pos],
        // Lava below or beside it meets it.
        Kind::Meets(_) => [[0, -1, 0], [0, 0, -1], [0, 0, 1], [1, 0, 0], [-1, 0, 0]]
            .iter()
            .map(|o| [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]])
            .filter(|p| matches!(fluid_at(level, *p), Some((Kind::Lava, _))))
            .collect(),
    };
    for p in lava {
        let Some((_, source)) = fluid_at(level, p) else {
            continue;
        };
        let Some(block) = meets(level, p) else {
            continue;
        };
        let becomes = if source { "minecraft:obsidian" } else { block };
        TURNING.with_borrow_mut(|t| t.push((dim, p, becomes)));
        if p == pos {
            event.cancelled = true;
        }
    }
}

/// The lava found meeting honey or chocolate turns, if it is still lava (`fizz`: level event
/// 1501, the lava's extinguish sound and smoke).
pub(super) fn tick(lv: &mut ServerLevel<'_>) {
    if KEYS.with_borrow(Vec::is_empty) {
        let keys: Vec<_> = IDS
            .iter()
            .filter_map(|id| Some((lv.host.fluid_key(id)?, *id)))
            .collect();
        KEYS.with_borrow_mut(|k| *k = keys);
    }
    let dim = lv.dim;
    let turning: Vec<_> = TURNING.with_borrow_mut(|t| {
        if t.is_empty() {
            return Vec::new();
        }
        let (here, rest): (Vec<_>, Vec<_>) = t.drain(..).partition(|e| e.0 == dim);
        *t = rest;
        here
    });
    for (_, pos, becomes) in turning {
        if !matches!(
            super::world_fluid(lv, pos),
            Some((create_engine::tank::Fluid::Lava, _))
        ) {
            continue;
        }
        let Some(state) = state_with(lv, becomes, &[]) else {
            continue;
        };
        lv.set_state(pos, state, all());
        lv.level.level_event(&mut *lv.host, 1501, pos, 0);
    }
}
