//! Create's fluids in the game: pipes and pumps, tanks and boilers, valves, smart, glass and
//! encased pipes, hose pulleys, creative fluid tanks, steam whistles, spouts and item drains.
//! The rules are `create_engine::{fluids, tank, steam, hose, whistle, plumbing, spout, drain}`.
//!
//! What clients draw and the goggles read comes in each block entity's data (`synced_keys`): a
//! tank controller's (or a creative tank's, spout's or drain's) contents under `Tank`, a boiler's
//! state, a hose pulley's hose, a pipe's rims, a drain's stack, an engine's shaft speed and a
//! spout's cycle; a valve's pointer follows the valve's own state and speed on the client.
//!
//! Left out:
//! - Potion fluids (`PotionFluid`): a spout fills glass bottles with water only, and drains and
//!   tanks take no potions.
//! - The `FLUIDS` capability on Create's tanks, so other mods' handlers and Create's pumps reach
//!   each other: the tanks' contents live in the engine's world, outside the game's transaction
//!   journal, and a handler cannot learn that a transaction it wrote in was aborted to roll the
//!   engine back (no close callback on `Transaction`). Pumps reach Create's own blocks and open
//!   ends as the legacy glue did.

mod clicks;
pub(crate) mod client;
mod drawn;
mod engine;
mod goggles;
mod lava;
mod machines;
mod pipes;
mod place;
mod plumbing;
mod rims;
mod spouted;

use super::server::ServerLevel;
use super::subsystem::Subsystem;
use super::world::with_world;
use create_engine::Block;
use create_engine::parts::StateInfo;
use create_engine::tank::{Fluid, MAX_HEIGHT, MAX_WIDTH, Multi, Tank};
use mod_api::{
    BlockState, BlockUpdateFlags, Compound, Flow, HostBlockStates, HostSide, ModBlockEntity,
    Registrar, RegistrationContext, RightClickBlockEvent, StateConstraint, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;

pub use client::animated;
pub(crate) use machines::insert_into_drain;

/// The key of a tank's contents in its block entity's data: fluid (255: none), amount, width,
/// height, window; empty for a tank block that is no controller.
pub const TANK: &str = "Tank";

/// The key of a hose pulley's hose: offset, blocks a tick it moves, the game time it was at.
pub const HOSE: &str = "Hose";

pub struct Fluids;

impl Subsystem for Fluids {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        client::register(r);
        for block in place::PLACED {
            r.block_part(super::states::key(block), place::Placement);
        }
        for block in clicks::CLICKED {
            r.block_part(super::states::key(block), clicks::Clicks);
        }
        let tank = r.item_key("create:fluid_tank");
        r.item_part(tank, place::TankItem);
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        let tanks = tanks_from_nbt(data);
        let dim = lv.dim;
        with_world(|w| w.load_tanks(dim, tanks.into_iter()));
        plumbing::chunk_load(dim, data);
        machines::chunk_load(dim, data);
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        let dim = lv.dim;
        let tanks = with_world(|w| w.column_tanks(dim, chunk[0], chunk[1])).unwrap_or_default();
        if !tanks.is_empty() {
            // x, y, z, fluid, amount, width, height, windows for each controller
            // (`FluidTankBlockEntity`'s `TankContent`, `Size`, `Height`, `Window`).
            let v = tanks
                .iter()
                .flat_map(|(p, m)| {
                    [
                        p[0],
                        p[1],
                        p[2],
                        i32::from(m.tank.fluid.map_or(255, |f| f.index())),
                        m.tank.amount as i32,
                        i32::from(m.width),
                        i32::from(m.height),
                        i32::from(m.window),
                    ]
                })
                .collect();
            data.insert("tank_multis".into(), Tag::IntArray(v));
        }
        plumbing::chunk_data_save(dim, chunk, data);
        machines::chunk_data_save(dim, chunk, data);
    }

    /// Before the solve, in the legacy order: pipes and pumps, boilers, valves, hoses and
    /// whistles, tank block states, spouts and drains. What clients must see again is `Tanks`',
    /// later.
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        lava::tick(lv);
        place::tick_layers(lv);
        pipes::tick_pipes(lv);
        pipes::tick_boilers(lv);
        plumbing::tick(lv);
        pipes::tick_tank_states(lv);
        machines::tick_spouts(lv);
        machines::tick_drains(lv);
        rims::tick(lv);
        drawn::send(lv);
        engine::tick(lv);
    }

    fn client_reset(&self) {
        client::reset();
    }

    /// Honey and chocolate meeting lava (`AllFluids.registerFluidInteractions`), looked at when
    /// either ticks (`lava`).
    fn fluid_tick(&self, level: &dyn mod_api::LevelHost, event: &mut mod_api::FluidTickEvent<'_>) {
        lava::fluid_tick(level, event);
    }

    /// The pipes beside a Create block that changed draw their pieces again (`rims`).
    fn block_changed(&self, dim: usize, pos: [i32; 3], _old: u32, new: u32) {
        engine::changed(dim, pos, new);
        rims::changed(dim, pos);
    }

    fn world_closed(&self) {
        lava::reset();
        place::reset();
        rims::reset();
        drawn::reset();
        engine::reset();
    }

    /// What clients see of tanks, spouts, drains, boilers and hoses, sent with their chunks too.
    fn synced_keys(&self) -> &'static [&'static str] {
        &[
            TANK,
            HOSE,
            BOILER,
            rims::RIMS,
            drawn::DRAIN_ITEM,
            engine::ENGINE_SPEED,
            plumbing::FILTER,
            plumbing::WHISTLE,
        ]
    }

    /// A powered shaft's engines draw its new speed.
    fn speed_changed(&self, lv: &mut ServerLevel<'_>, pos: [i32; 3], speed: f32) {
        engine::shaft_speed_changed(lv, pos, speed);
    }

    /// The client's prediction of a click its block takes (`clicks`).
    fn right_click_block(&self, host: HostSide, event: &mut RightClickBlockEvent<'_>) -> Flow {
        match host {
            HostSide::Client(host) => clicks::predict(host, event),
            HostSide::Server(_) => Flow::Continue,
        }
    }

    /// Tanks' and boilers', spouts' and drains' goggle lines (`goggles`).
    fn goggle_tooltip(
        &self,
        host: &dyn mod_api::ClientHost,
        pos: [i32; 3],
        block: Block,
        lines: &mut Vec<Vec<(String, u32)>>,
    ) {
        goggles::tooltip(host, pos, block, lines);
    }

    /// The wrench on a tank, a pipe or a whistle, not sneaking (legacy `use_item_on`'s
    /// `wrench_pipe` / `wrench_whistle`, after a bracket came off: `decor`'s part).
    fn wrench_used(
        &self,
        lv: &mut ServerLevel<'_>,
        _player: mod_api::PlayerRef,
        pos: [i32; 3],
        block: Block,
        clicked: create_engine::kinetics::Dir,
        point: [f64; 3],
        sneaking: bool,
    ) -> bool {
        if sneaking {
            return false;
        }
        // `FluidTankBlock.onWrenched`: the tank's windows on or off (none on an active boiler).
        if matches!(block, Block::FluidTank | Block::CreativeFluidTank) {
            let dim = lv.dim;
            with_world(|w| w.toggle_tank_windows(dim, pos));
            return true;
        }
        plumbing::wrench_pipe(lv, pos, block, clicked, point)
            || plumbing::wrench_whistle(lv, pos, block)
    }

    /// A spout two above belt segment `segment` (`SpoutBlockEntity`'s
    /// `BeltProcessingBehaviour`); the deployers and presses are processing's.
    fn belt_process(
        &self,
        lv: &mut ServerLevel<'_>,
        segment: [i32; 3],
        item: &mut create_engine::belt::Item,
        locked: bool,
    ) -> Option<create_engine::belt::Process> {
        let above = [segment[0], segment[1] + 2, segment[2]];
        machines::spout_on_belt(lv, above, item, locked)
    }
}

/// The tanks whose contents changed this tick, to their comparators and clients (legacy
/// `send_tank_changes`): after redstone's `Detectors`, which read the changes, and before the
/// solve.
pub struct Tanks;

impl Subsystem for Tanks {
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        send_tank_changes(lv);
        send_shown(lv);
    }

    fn world_closed(&self) {
        SHOWN.with_borrow_mut(HashMap::clear);
    }
}

/// The key of a boiler's state in its controller's block entity data, for the goggles: active
/// heat, passive (0 or 1), water supply (`f32` bits), engines, whistles.
pub const BOILER: &str = "Boiler";

/// The key of the game time a spout's filling cycle started, while it runs, for its clients.
pub const SPOUT_START: &str = "SpoutStart";

/// A block entity's key by level and position.
type ShownKey = (usize, [i32; 3], &'static str);

thread_local! {
    /// What each spout's, drain's and boiler's block entity last showed its clients, by level
    /// and position.
    static SHOWN: RefCell<HashMap<ShownKey, Vec<i32>>> =
        RefCell::new(HashMap::new());
}

/// Spouts' and item drains' contents (fluid, amount, capacity, under `TANK`) and boilers'
/// states (`BOILER`) to their block entities' clients when they changed (`notifyUpdate`).
fn send_shown(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let time = lv.game_time();
    let now: Vec<([i32; 3], &'static str, Vec<i32>)> = with_world(|w| {
        let d = &w.dims[dim];
        let fluid = |f: Option<(Fluid, u32)>, capacity: u32| match f {
            Some((f, n)) => vec![i32::from(f.index()), n as i32, capacity as i32],
            None => vec![255, 0, capacity as i32],
        };
        let spouts = d
            .spouts
            .iter()
            .map(|(p, s)| (*p, TANK, fluid(s.fluid, create_engine::spout::CAPACITY)));
        let drains = d
            .drains
            .iter()
            .map(|(p, s)| (*p, TANK, fluid(s.fluid, create_engine::drain::CAPACITY)));
        let boilers = d.boilers.iter().map(|(p, b)| {
            let width = d.tanks.multis.get(p).map_or(1, |m| i32::from(m.width));
            let b = &b.data;
            let v = vec![
                b.active_heat,
                i32::from(b.passive_heat),
                b.water_supply.to_bits() as i32,
                b.attached_engines,
                b.attached_whistles,
                // The tank's width, for its sides' occlusion below.
                width,
            ];
            (*p, BOILER, v)
        });
        // A spout working shows when its cycle started (`processingTicks`).
        let working = d
            .spouts
            .iter()
            .filter(|(_, s)| s.processing >= 0)
            .map(|(p, s)| {
                let start = time - i64::from(create_engine::spout::FILLING_TIME - s.processing);
                (*p, SPOUT_START, vec![start as i32])
            });
        spouts.chain(drains).chain(boilers).chain(working).collect()
    })
    .unwrap_or_default();
    // A boiler's sides its gauges are hidden on (`updateOcclusion`: something solid beside the
    // middle of its bottom layer), in place of its width.
    let now: Vec<_> = now
        .into_iter()
        .map(|(p, k, mut v)| {
            if k == BOILER
                && let Some(width) = v.pop()
            {
                v.push(boiler_occlusion(lv, p, width));
            }
            (p, k, v)
        })
        .collect();
    let changed: Vec<_> = SHOWN.with_borrow_mut(|shown| {
        // Gone (a boiler no more, a drain broken): shown as nothing.
        let mut changed: Vec<_> = shown
            .keys()
            .filter(|(d, p, k)| *d == dim && !now.iter().any(|(q, j, _)| q == p && j == k))
            .map(|(_, p, k)| (*p, *k, Vec::new()))
            .collect();
        for (p, k, _) in &changed {
            shown.remove(&(dim, *p, *k));
        }
        for (p, k, v) in now {
            if shown.get(&(dim, p, k)) != Some(&v) {
                shown.insert((dim, p, k), v.clone());
                changed.push((p, k, v));
            }
        }
        changed
    });
    for (pos, key, v) in changed {
        // A spout's or drain's contents are saved with its column.
        if key == TANK {
            with_world(|w| w.mark_unsaved(dim, pos));
        }
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let data = Compound::from([(key.into(), Tag::IntArray(v))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
}

/// `BoilerData.updateOcclusion` on the server: a bit per horizontal side (`get2DDataValue`:
/// south, west, north, east) where a block with collision stands beside the middle of the
/// boiler's bottom layer (its middle block, or two for an even width), hiding that side's gauge.
fn boiler_occlusion(lv: &ServerLevel<'_>, c: [i32; 3], width: i32) -> i32 {
    let middle: Vec<i32> = if width % 2 == 1 {
        vec![width / 2]
    } else {
        vec![width / 2 - 1, width / 2]
    };
    let sides: [(i32, i32); 4] = [(0, 1), (-1, 0), (0, -1), (1, 0)];
    let mut bits = 0;
    for (i, (dx, dz)) in sides.into_iter().enumerate() {
        let hidden = middle.iter().any(|m| {
            let (x, z) = match (dx, dz) {
                (1, _) => (width, *m),
                (-1, _) => (-1, *m),
                (_, 1) => (*m, width),
                _ => (*m, -1),
            };
            crate::glue::logistics::has_collision(lv, [c[0] + x, c[1], c[2] + z])
        });
        if hidden {
            bits |= 1 << i;
        }
    }
    bits
}

/// Tanks saved with a column: `tank_multis` holds x, y, z, fluid, amount, width, height,
/// windows for each controller; `tanks` (before multiblock tanks) x, y, z, fluid, amount for
/// one-block tanks.
fn tanks_from_nbt(data: &Compound) -> Vec<([i32; 3], Multi)> {
    let tank = |c: &[i32]| Tank {
        fluid: Fluid::from_index(c[3].clamp(0, 255) as u8),
        amount: c[4].max(0) as u32,
    };
    let mut out = Vec::new();
    if let Some(Tag::IntArray(v)) = data.get(&"tank_multis".into()) {
        out.extend(v.as_chunks::<8>().0.iter().map(|c| {
            (
                [c[0], c[1], c[2]],
                Multi {
                    tank: tank(c),
                    width: c[5].clamp(1, MAX_WIDTH) as u8,
                    height: c[6].clamp(1, MAX_HEIGHT) as u8,
                    window: c[7] != 0,
                },
            )
        }));
    }
    if let Some(Tag::IntArray(v)) = data.get(&"tanks".into()) {
        out.extend(v.as_chunks::<5>().0.iter().map(|c| {
            (
                [c[0], c[1], c[2]],
                Multi {
                    tank: tank(c),
                    ..Multi::default()
                },
            )
        }));
    }
    out
}

/// Tanks whose contents changed: their comparators read them again (`onFluidStackChanged`:
/// `setChanged`), and each controller's block entity shows its clients the tank (none for a
/// block that is no controller anymore).
fn send_tank_changes(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let changed = with_world(|w| {
        let mut v = std::mem::take(&mut w.dims[dim].tank_changes);
        v.sort();
        v.dedup();
        w.dims[dim].analog_checks.extend_from_slice(&v);
        v.into_iter()
            .map(|p| (p, w.dims[dim].tanks.multis.get(&p).copied()))
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    for (pos, m) in changed {
        set_shown_tank(lv, pos, m.as_ref());
    }
}

/// Puts tank `m` (none: no tank) in the data of the block entity at `pos`, for its clients.
fn set_shown_tank(lv: &mut ServerLevel<'_>, pos: [i32; 3], m: Option<&Multi>) {
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let v = m.map_or_else(Vec::new, |m| {
        vec![
            i32::from(m.tank.fluid.map_or(255, Fluid::index)),
            m.tank.amount as i32,
            i32::from(m.width),
            i32::from(m.height),
            i32::from(m.window),
        ]
    });
    let data = Compound::from([(TANK.into(), Tag::IntArray(v))]);
    let _ = entity.merge_data(&mut *lv.host, data);
}

/// The tank a block entity shows its clients.
fn shown_tank(entity: &ModBlockEntity) -> Option<Multi> {
    tank_in(&entity.nbt)
}

/// The tank block entity data `data` shows.
fn tank_in(data: &Compound) -> Option<Multi> {
    let Some(Tag::IntArray(v)) = data.get(&TANK.into()) else {
        return None;
    };
    let &[fluid, amount, width, height, window] = v.as_slice() else {
        return None;
    };
    Some(Multi {
        tank: Tank {
            fluid: Fluid::from_index(fluid.clamp(0, 255) as u8),
            amount: amount.max(0) as u32,
        },
        width: width.clamp(1, MAX_WIDTH) as u8,
        height: height.clamp(1, MAX_HEIGHT) as u8,
        window: window != 0,
    })
}

/// The Create block at `pos` and its state's facts.
fn info_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(Block, StateInfo)> {
    let state = lv.block(pos);
    if state == 0 {
        return None;
    }
    with_world(|w| w.table.get(state).map(|(b, i)| (b, *i))).flatten()
}

/// The state of block `name` with `props`, the rest default.
fn state_with(lv: &ServerLevel<'_>, name: &str, props: &[(&str, &str)]) -> Option<BlockState> {
    let key = lv.host.block_key(name)?;
    let constraints: Vec<StateConstraint> = props
        .iter()
        .map(|(k, v)| StateConstraint::new(*k, *v))
        .collect();
    BlockState::of(&lv.host.block_states(), key, &constraints).ok()
}

/// A property of the state at `pos` (`lit`, `waterlogged`).
fn property(lv: &ServerLevel<'_>, pos: [i32; 3], key: &str) -> Option<String> {
    lv.level.block_property(&*lv.host, pos, key)
}

/// `BlockState.canBeReplaced()` of the state at `pos`.
fn replaceable(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    lv.block_state(pos)
        .and_then(|s| lv.host.block_states().replaceable(s))
        .unwrap_or(false)
}

/// The fluid in the block at `pos`: water, lava, honey or chocolate, and whether it is a
/// source. None for no fluid or another.
fn world_fluid(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(Fluid, bool)> {
    let f = lv.level.fluid_state(&*lv.host, pos)?;
    if f.is_empty() {
        return None;
    }
    let fluid = match f.fluid.id(&*lv.host).as_str() {
        "minecraft:water" | "minecraft:flowing_water" => Fluid::Water,
        "minecraft:lava" | "minecraft:flowing_lava" => Fluid::Lava,
        "create:honey" | "create:flowing_honey" => Fluid::Honey,
        "create:chocolate" | "create:flowing_chocolate" => Fluid::Chocolate,
        _ => return None,
    };
    Some((fluid, f.is_source()))
}

/// Any fluid at all in the block at `pos` (`!getFluidState().isEmpty()`).
fn has_fluid(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    lv.level
        .fluid_state(&*lv.host, pos)
        .is_some_and(|f| !f.is_empty())
}

fn offset(p: [i32; 3], d: create_engine::kinetics::Dir) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

fn flag(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// Vanilla's `Block.UPDATE_ALL`.
fn all() -> BlockUpdateFlags {
    super::server::update_all()
}
