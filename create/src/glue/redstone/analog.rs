//! What comparators read of Create's blocks (`hasAnalogOutputSignal`, `getAnalogOutputSignal`),
//! as each block's class computes it.
//!
//! The game asks from inside its update, where only the block's own state may be read: a block
//! whose output is in the engine's state (a tank, a gauge, a burner's heat) answers from it at
//! once; one whose output is its item slots (a vault, a depot, a basin, a toolbox) answers from
//! the level last read of its slots, read again when its contents change (before the game tells
//! its comparators) and after each level tick for the blocks marked (`analog_touched`), when the
//! comparators beside them are told (`Comparators`, last in the level tick).
//!
//! Packagers, repackagers, redstone requesters, frogports and postboxes keep their packages
//! typed: their output is the packages port's.

use super::detectors::{max_stack, slot_stack};
use super::info;
use super::parts::dim_of;
use crate::glue::server::ServerLevel;
use crate::glue::states;
use crate::glue::subsystem::Subsystem;
use crate::glue::world::{try_with_world, with_world};
use create_engine::Block;
use create_engine::comparator as cmp;
use create_engine::parts::StateInfo;
use mod_api::{
    AnalogOutputEvent, BlockPart, ContentsChangedEvent, HasAnalogOutputEvent, LevelHost, Registrar,
    ServerHost,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// Whether the packages port answers for `b`.
fn packaged(b: Block) -> bool {
    matches!(
        b,
        Block::Packager | Block::Repackager | Block::RedstoneRequester | Block::PackageFrogport
    ) || create_engine::dye::is_postbox(b)
}

pub(super) fn register(r: &mut dyn Registrar) {
    for &block in Block::ALL.iter() {
        if cmp::has_analog_output(block) && !packaged(block) {
            r.block_part(states::key(block), AnalogBlock);
        }
        if create_engine::world::keeps_state(block) {
            r.block_part(states::key(block), ContentsBlock);
        }
    }
}

thread_local! {
    /// The level last read of each slot block's items, by level and position.
    static LEVELS: RefCell<HashMap<(usize, [i32; 3]), u8>> = RefCell::new(HashMap::new());
    /// Blocks asked for before their slots were read, and blocks whose contents changed: read
    /// (and their detectors told) after the level's tick.
    static TOUCHED: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
}

/// Forgets every reading (the world closed).
pub(super) fn forget() {
    LEVELS.with_borrow_mut(HashMap::clear);
    TOUCHED.with_borrow_mut(Vec::clear);
}

/// The block at `pos` of level `dim` may read differently to comparators: looked at after the
/// level's tick.
pub fn analog_touched(dim: usize, pos: [i32; 3]) {
    if try_with_world(|_| ()).is_some() {
        with_world(|w| w.dims[dim].analog_checks.push(pos));
    } else {
        TOUCHED.with_borrow_mut(|t| t.push((dim, pos)));
    }
}

/// What the block's own state gives, read without its slots.
enum Typed {
    Level(i32),
    /// Its slots: read with the host.
    Slots,
}

/// The output of `block` at `pos` that the engine's state gives (`Typed::Slots`: its slots').
fn typed(dim: usize, pos: [i32; 3], block: Block, info: &StateInfo) -> Option<Typed> {
    if !cmp::has_analog_output(block) || packaged(block) {
        return None;
    }
    try_with_world(|w| {
        let d = &w.dims[dim];
        match block {
            Block::FluidTank => Typed::Level(
                d.tanks
                    .get(pos)
                    .map_or(0, |(_, m)| cmp::fluid_tank(m.tank.amount, m.capacity())),
            ),
            // A creative tank reads full of its fluid, or empty.
            Block::CreativeFluidTank => {
                let full = d.plumbing.creative_fluid(pos).flatten().is_some();
                let cap = create_engine::tank::CAPACITY;
                Typed::Level(cmp::fluid_tank(if full { cap } else { 0 }, cap))
            }
            Block::Spout => Typed::Level(cmp::smart_tank(
                d.spouts.get(&pos).and_then(|s| s.fluid).map_or(0, |f| f.1),
                create_engine::spout::CAPACITY,
            )),
            Block::ItemDrain => Typed::Level(cmp::smart_tank(
                d.drains.get(&pos).and_then(|s| s.fluid).map_or(0, |f| f.1),
                create_engine::drain::CAPACITY,
            )),
            Block::CopperBacktank | Block::NetheriteBacktank => {
                Typed::Level(d.backtanks.get(&pos).map_or(0, |t| t.comparator()))
            }
            Block::Speedometer | Block::Stressometer => Typed::Level(
                d.gauges
                    .get(&pos)
                    .map_or(0, |g| create_engine::gauge::analog_output(g.shown.dial)),
            ),
            Block::SequencedGearshift => Typed::Level(i32::from(info.sequence)),
            Block::BlazeBurner => Typed::Level(info.heat.map_or(0, cmp::blaze_burner)),
            Block::LitBlazeBurner => Typed::Level(cmp::lit_blaze_burner(info.soul)),
            Block::TrackSignal => Typed::Level(cmp::full(
                d.railway.lights.get(&pos) == Some(&create_engine::trains::SignalState::Red),
            )),
            Block::TrackStation => Typed::Level(cmp::full(
                d.railway
                    .trains
                    .iter()
                    .any(|t| t.train.at_station == Some(pos)),
            )),
            // `LecternControllerBlock.getAnalogOutputSignal`: always 15.
            Block::LecternController => Typed::Level(15),
            // `ControllerRailBlock.getAnalogOutputSignal`: its power.
            Block::ControllerRail => Typed::Level(i32::from(info.power)),
            // `PortableStorageInterfaceBlock`: 15 while connected.
            Block::PortableStorageInterface | Block::PortableFluidInterface => Typed::Level(
                cmp::full(d.psis.get(&pos).is_some_and(|s| s.is_connected())),
            ),
            _ => Typed::Slots,
        }
    })
}

/// The output of the slot block `block` at `pos`, from its slots.
fn slots_level(lv: &mut ServerLevel<'_>, pos: [i32; 3], block: Block) -> i32 {
    let dim = lv.dim;
    // `calcRedstoneFromInventory` over the slots `range` of each block, 64 to a slot.
    let inventory = |lv: &ServerLevel<'_>, blocks: &[[i32; 3]], range: std::ops::Range<usize>| {
        let mut slots = Vec::with_capacity(blocks.len() * range.len());
        for &p in blocks {
            for i in range.clone() {
                slots.push(slot_stack(lv, p, i as u32).map(|s| (s.count(), max_stack(lv, &s))));
            }
        }
        cmp::inventory(slots, 64)
    };
    // `DepotBehaviour.getPresentStackSize`: the held stack and the outputs.
    let present = |lv: &ServerLevel<'_>| -> u32 {
        (0..create_engine::depot::SLOTS as u32)
            .filter_map(|i| slot_stack(lv, pos, i))
            .map(|s| s.count())
            .sum()
    };
    match block {
        Block::ItemVault => {
            let blocks = try_with_world(|w| w.dims[dim].vaults.inventory(pos))
                .flatten()
                .unwrap_or_else(|| vec![pos]);
            inventory(lv, &blocks, 0..create_engine::vault::SLOTS)
        }
        Block::WeightedEjector => {
            let max = try_with_world(|w| {
                w.dims[dim]
                    .ejectors
                    .get(&pos)
                    .map(|e| u32::from(e.max_stack))
            })
            .flatten()
            .unwrap_or(0);
            cmp::depot(present(lv), max)
        }
        // A depot: its items over the held stack's max stack size, 64 when it holds none.
        Block::Depot => {
            let held = slot_stack(lv, pos, 0).map_or(64, |s| max_stack(lv, &s));
            cmp::depot(present(lv), held)
        }
        Block::Basin => inventory(lv, &[pos], create_engine::basin::INPUTS),
        b if create_engine::dye::is_toolbox(b) => {
            inventory(lv, &[pos], 0..create_engine::toolbox::STORED)
        }
        _ => 0,
    }
}

/// The output of the Create block at `pos` of `lv` that has one, as its class computes it,
/// reading its slots; None for any other block.
fn analog_level(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> Option<i32> {
    let (block, info) = super::block_at(lv, pos)?;
    match typed(lv.dim, pos, block, &info)? {
        Typed::Level(l) => Some(l),
        Typed::Slots => {
            let level = slots_level(lv, pos, block).clamp(0, 15) as u8;
            LEVELS.with_borrow_mut(|m| m.insert((lv.dim, pos), level));
            Some(i32::from(level))
        }
    }
}

struct AnalogBlock;

#[mod_api::horizon_part]
impl BlockPart for AnalogBlock {
    fn has_analog_output(&self, _level: &dyn LevelHost, event: &mut HasAnalogOutputEvent) {
        if states::id(event.state).is_some() {
            event.output = true;
        }
    }

    /// `getAnalogOutputSignal`: no Create block reads differently by side.
    fn analog_output(&self, _level: &dyn LevelHost, event: &mut AnalogOutputEvent) {
        let Some((block, info)) = states::id(event.state).and_then(info) else {
            return;
        };
        let Some(dim) = dim_of(event.dimension.key().id()) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        match typed(dim, pos, block, &info) {
            Some(Typed::Level(l)) => event.output = l,
            Some(Typed::Slots) => match LEVELS.with_borrow(|m| m.get(&(dim, pos)).copied()) {
                Some(l) => event.output = i32::from(l),
                None => {
                    // Not read yet: read after the tick, its comparators told then.
                    event.output = 0;
                    TOUCHED.with_borrow_mut(|t| t.push((dim, pos)));
                }
            },
            None => {}
        }
    }
}

/// A Create block's contents changed (`setChanged`), before the game tells its comparators:
/// its level is read again now; smart observers and threshold switches watching it look again,
/// and a vault's other blocks' comparators read the whole vault again
/// (`ItemVaultBlockEntity.updateComparators`), after the tick.
struct ContentsBlock;

#[mod_api::horizon_part]
impl BlockPart for ContentsBlock {
    fn contents_changed(&self, host: &mut dyn ServerHost, event: &mut ContentsChangedEvent) {
        let Some((block, _)) = states::id(event.state).and_then(info) else {
            return;
        };
        let Some(dimension) = event.level.dimension().id(&*host) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        let dim = lv.dim;
        TOUCHED.with_borrow_mut(|t| t.push((dim, pos)));
        if cmp::has_analog_output(block) && !packaged(block) {
            analog_level(&mut lv, pos);
        }
    }
}

/// The blocks touched since the last tick: the detectors watching them look again, their slots
/// are read after the tick.
pub(super) fn take_touched(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    if TOUCHED.with_borrow(Vec::is_empty) {
        return;
    }
    let mine: Vec<[i32; 3]> = TOUCHED.with_borrow_mut(|t| {
        let (mine, rest): (Vec<_>, Vec<_>) =
            std::mem::take(t).into_iter().partition(|e| e.0 == dim);
        *t = rest;
        mine.into_iter().map(|e| e.1).collect()
    });
    with_world(|w| {
        for pos in mine {
            w.detectors_contents_changed(dim, pos);
            w.dims[dim].analog_checks.push(pos);
        }
    });
}

/// A block of `dim` changed: its slots' level is read again when asked.
pub(super) fn block_changed(dim: usize, pos: [i32; 3]) {
    LEVELS.with_borrow_mut(|m| m.remove(&(dim, pos)));
}

/// A column left: its readings go.
pub(super) fn column_unloaded(dim: usize, chunk: [i32; 2]) {
    LEVELS.with_borrow_mut(|m| {
        m.retain(|(d, p), _| *d != dim || [p[0] >> 4, p[2] >> 4] != chunk);
    });
}

/// Comparators, last in a level's tick.
pub struct Comparators;

impl Subsystem for Comparators {
    /// After a level's tick: the blocks whose comparator output may have changed are read again
    /// and comparators beside them told (`updateNeighbourForOutputSignal`); fluid blocks only
    /// when it moved. Nothing runs while no block changed.
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        tick_comparators(lv);
    }
}

fn tick_comparators(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let Some(blocks) = with_world(|w| {
        if w.dims[dim].analog_checks.is_empty() {
            return None;
        }
        let checks = std::mem::take(&mut w.dims[dim].analog_checks);
        let mut all: Vec<[i32; 3]> = checks
            .into_iter()
            .flat_map(|p| w.analog_unit(dim, p))
            .collect();
        all.sort_unstable();
        all.dedup();
        Some(all)
    })
    .flatten() else {
        return;
    };
    for pos in blocks {
        if !lv.is_loaded(pos) {
            continue;
        }
        let Some(level) = analog_level(lv, pos) else {
            continue;
        };
        let level = level.clamp(0, 15) as u8;
        let state = lv.block(pos);
        let tell = with_world(|w| {
            let fluid = w.table.get(state).is_some_and(|(b, _)| {
                matches!(b, Block::FluidTank | Block::Spout | Block::ItemDrain)
            });
            !fluid || w.dims[dim].analog_levels.insert(pos, level) != Some(level)
        })
        .unwrap_or(false);
        if tell {
            lv.update_output_signal(pos);
        }
    }
}
