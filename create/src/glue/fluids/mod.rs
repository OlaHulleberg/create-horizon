//! Create's fluids in the game: pipes and pumps, tanks and boilers, valves, smart, glass and
//! encased pipes, hose pulleys, creative fluid tanks, steam whistles, spouts and item drains.
//! The rules are `create_engine::{fluids, tank, steam, hose, whistle, plumbing, spout, drain}`.
//!
//! What clients draw and the goggles read comes in each block entity's data (`synced_keys`): a
//! tank controller's (or a creative tank's, spout's or drain's) contents under `Tank`, a boiler's
//! state, a hose pulley's hose, a pipe's rims, a drain's stack, an engine's shaft speed and a
//! spout's cycle; a valve's pointer follows the valve's own state and speed on the client.
//!
//! Potions and builder's tea are fluids too (`tank::Fluid::Potion`, `Tea`): drains, tanks and
//! basins take potions as their potion fluid (a regular water bottle as water), spouts fill
//! bottles with it; the engine names a potion item with its contents (`tank::potion_item`). A
//! potion fluid carries its whole `potion_contents` (custom effects, colour and name:
//! `create_engine::potion::Contents`) and its bottle, so potions of other contents never mix; the
//! int arrays fluids are saved and sent in number a potion by its place in a list of keys kept
//! beside them (`tank::Codes`, `fluid_keys`).
//!
//! Creative fluid tanks join into tanks as fluid tanks do (`creative_tanks` in the engine's
//! world), each block giving the tank's fluid (`plumbing.creative`). Steam engines hiss and puff
//! at each stroke (`engine`'s client tick), and boiler gauges swing to their level (`client`).
//! An engine turns its shaft clockwise or counter-clockwise as its value box says
//! (`movementDirection`, kept as a contraption block's option; `interaction::values`), the box
//! inactive and the direction turned round to match while another source turns the shaft.
//!
//! A pump's flows grow through its pipes connection by connection and recede from the source
//! (`flows`): glass pipes draw them, pipes splash, drip and pour them on their clients. Flows of
//! two fluids meeting in a pipe break it into cobblestone, limestone or scoria
//! (`handlePipeFlowCollision`). Each open end keeps its own tank (`OpenEndedPipe`, saved with
//! its column): a bucket's worth of a fluid with a block becomes a source (water in a
//! waterloggable block waterlogs it, and boils away where water evaporates), one without (milk,
//! tea, potions) is taken a drop at a time, and each gives what is in front its effect (milk
//! clears every effect, as vanilla's milk bucket does: its `minecraft:consumable` component's
//! `clear_all_effects` in the game's 26.3 item data; a potion its contents' custom
//! effects too: `open_end`). Open ends draw from sources, waterlogged blocks, full cauldrons and
//! beehives. Smart observers and threshold switches read the tanks of every fluid handler
//! (`fluid_handler_tanks`) and an observer the flows in a pipe (`pipe_inflows`).
//!
//! Left out:
//! - The `FLUIDS` capability on Create's tanks, so other mods' handlers and Create's pumps reach
//!   each other: the tanks' contents live in the engine's world, outside the game's transaction
//!   journal, and a handler cannot learn that a transaction it wrote in was aborted to roll the
//!   engine back (no close callback on `Transaction`). Pumps reach Create's own blocks and open
//!   ends.

mod carried;
pub(crate) mod clicks;
pub(crate) mod client;
mod drawn;
pub(crate) mod engine;
pub(crate) use engine::engine_sourced;
mod connections;
mod flows;
mod goggles;
mod lava;
pub(crate) mod machines;
mod open_end;
pub(crate) use open_end::drink_potion;
pub(crate) mod pipes;
mod place;
pub(crate) mod plumbing;
mod rims;
mod spouted;

use super::server::ServerLevel;
use super::subsystem::Subsystem;
use super::world::with_world;
use create_engine::Block;
use create_engine::parts::StateInfo;
use create_engine::tank::{Codes, Fluid, MAX_HEIGHT, MAX_WIDTH, Multi, Tank};
use mod_api::{
    BlockState, BlockUpdateFlags, Compound, Flow, HostBlockStates, HostSide, ModBlockEntity,
    Registrar, RegistrationContext, RightClickBlockEvent, StateConstraint, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;

pub use client::animated;
pub(crate) use machines::insert_into_drain_sim;

/// Whether emptying `item` gives water (`GenericItemEmptying.emptyItem(..).getFluid()`): a
/// water bucket, a water bottle.
pub(crate) fn empties_water(item: &str) -> bool {
    clicks::emptying(item).is_some_and(|e| e.fluid == Fluid::Water)
}

/// What a smart observer looking at the pipe-like block at `pos` sees of its flows
/// (`FluidTransportBehaviour.getFlow`: each side's inbound, complete flow); None for a block
/// with no pipe behaviour (pipes, encased, glass and smart pipes, valves and pumps have one).
pub(crate) fn pipe_inflows(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Vec<Fluid>> {
    let (block, _) = info_at(lv, pos)?;
    if !matches!(
        block,
        Block::FluidPipe
            | Block::EncasedFluidPipe
            | Block::GlassFluidPipe
            | Block::SmartFluidPipe
            | Block::FluidValve
            | Block::MechanicalPump
    ) {
        return None;
    }
    let flows = flows::provided(lv.dim, pos, lv.game_time());
    Some(flows.into_iter().map(|(_, f)| f).collect())
}

/// The tanks of the fluid handler of Create's block at `pos` as a smart observer or threshold
/// switch reads them (`IFluidHandler.getTanks`); None for a block with none. A tank (its whole
/// multiblock) or a boiler (`BoilerFluidHandler`: one empty tank of 10000 that gives nothing), a
/// creative tank (always full), a basin's outputs then inputs (`CombinedTankWrapper`), a spout's,
/// drain's and hose pulley's tank, a portable fluid interface's contraption's tanks.
pub(crate) fn fluid_handler_tanks(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
) -> Option<Vec<create_engine::observe::TankSlot>> {
    use create_engine::observe::TankSlot;
    let (block, _) = info_at(lv, pos)?;
    let dim = lv.dim;
    match block {
        Block::FluidTank => with_world(|w| {
            if w.is_boiler(dim, pos) {
                let boiler = TankSlot {
                    drains: false,
                    ..TankSlot::of(None, 10000)
                };
                return Some(vec![boiler]);
            }
            let (_, m) = w.dims[dim].tanks.get(pos)?;
            let held = m.tank.fluid.map(|f| (f, m.tank.amount));
            Some(vec![TankSlot::of(held, m.capacity())])
        })
        .flatten(),
        Block::CreativeFluidTank => with_world(|w| {
            let d = &w.dims[dim];
            let cap = d
                .creative_tanks
                .get(pos)
                .map_or(create_engine::tank::CAPACITY, |(_, m)| m.capacity());
            let fluid = d.plumbing.creative_fluid(pos).flatten();
            Some(vec![TankSlot::of(fluid.map(|f| (f, cap)), cap)])
        })
        .flatten(),
        Block::Basin => {
            let t = super::processing::basin_tanks(dim, pos);
            let cap = create_engine::basin::TANK;
            Some(
                t.output
                    .iter()
                    .chain(&t.input)
                    .map(|f| TankSlot::of(*f, cap))
                    .collect(),
            )
        }
        Block::Spout => with_world(|w| {
            let fluid = w.dims[dim].spouts.get(&pos).and_then(|s| s.fluid);
            vec![TankSlot::of(fluid, create_engine::spout::CAPACITY)]
        }),
        Block::ItemDrain => with_world(|w| {
            let fluid = w.dims[dim].drains.get(&pos).and_then(|s| s.fluid);
            vec![TankSlot::of(fluid, create_engine::drain::CAPACITY)]
        }),
        Block::HosePulley => with_world(|w| {
            let tank = w.dims[dim].plumbing.hoses.get(&pos).map(|h| h.tank);
            let fluid = tank.and_then(|t| t.fluid.map(|f| (f, t.amount)));
            vec![TankSlot::of(fluid, create_engine::hose::CAPACITY)]
        }),
        Block::PortableFluidInterface => crate::glue::contraptions::actors::pfi_tanks(lv, pos),
        _ => None,
    }
}

/// The key of a tank's contents in its block entity's data: fluid (255: none), amount, width,
/// height, window; empty for a tank block that is no controller. A potion's number names its
/// key in the list beside it (`fluid_keys`).
pub const TANK: &str = "Tank";
/// The keys of the fluids `TANK` numbers (`fluid_keys`).
pub const TANK_FLUIDS: &str = "Tank_fluids";

/// Where the keys of the fluids an int array numbers (`tank::Codes`, a potion with its whole
/// contents) are kept beside it: the array's key and `_fluids`.
fn keys_key(array: &str) -> String {
    format!("{array}_fluids")
}

/// Puts the keys `codes` gave numbers beside array `array` in `data` (nothing when none).
pub(crate) fn put_fluid_keys(data: &mut Compound, array: &str, codes: Codes) {
    if codes.keys.is_empty() {
        return;
    }
    let list = codes
        .keys
        .into_iter()
        .map(|k| Tag::String(k.as_str().into()))
        .collect();
    data.insert(keys_key(array).as_str().into(), Tag::List(list));
}

/// The fluid keys kept beside array `array` in `data`.
pub(crate) fn fluid_keys(data: &Compound, array: &str) -> Vec<String> {
    match data.get(&keys_key(array).as_str().into()) {
        Some(Tag::List(l)) => l
            .iter()
            .map(|t| match t {
                Tag::String(s) => s.to_string_lossy(),
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The key of a hose pulley's hose: offset, blocks a tick it moves, the game time it was at.
pub const HOSE: &str = "Hose";

pub struct Fluids;

impl Subsystem for Fluids {
    /// The steam engines' and whistles' jets (`engine::render_jets`).
    fn render_level_stage(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::RenderLevelStageEvent<'_>,
    ) {
        engine::render_jets(host, event);
    }

    fn draw_carried(
        &self,
        host: &dyn mod_api::ClientHost,
        block: create_engine::Block,
        event: &mut mod_api::BlockEntitySubmitEvent<'_>,
    ) {
        client::draw_carried(host, block, event);
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

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        client::register(r);
        flows::register(r);
        for block in place::PLACED {
            r.block_part(super::states::key(block), place::Placement);
        }
        for block in clicks::CLICKED {
            r.block_part(super::states::key(block), clicks::Clicks);
        }
        let tank = r.item_key("create:fluid_tank");
        r.item_part(tank, place::TankItem);
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, chunk: [i32; 2], data: &Compound) {
        flows::forget_column(lv.dim, chunk);
        let tanks = tanks_from_nbt(data);
        let dim = lv.dim;
        with_world(|w| w.load_tanks(dim, tanks.into_iter()));
        plumbing::chunk_load(dim, data);
        machines::chunk_load(dim, data);
        pipes::chunk_load(dim, data);
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        let dim = lv.dim;
        let tanks = with_world(|w| w.column_tanks(dim, chunk[0], chunk[1])).unwrap_or_default();
        if !tanks.is_empty() {
            // Per-member FluidTankBlockEntity.write state, including Uninitialized;
            // controller contents occupy the first eight fields.
            let mut codes = Codes::default();
            let v = tanks
                .iter()
                .flat_map(|(p, saved)| {
                    let m = saved.multi.unwrap_or_default();
                    [
                        p[0],
                        p[1],
                        p[2],
                        codes.code(m.tank.fluid),
                        m.tank.amount as i32,
                        i32::from(m.width),
                        i32::from(m.height),
                        i32::from(m.window),
                        saved.part.controller[0],
                        saved.part.controller[1],
                        saved.part.controller[2],
                        i32::from(saved.part.window),
                        i32::from(saved.part.update_connectivity),
                        i32::from(saved.multi.is_some()),
                    ]
                })
                .collect();
            data.insert("tank_entities".into(), Tag::IntArray(v));
            put_fluid_keys(data, "tank_entities", codes);
        }
        plumbing::chunk_data_save(dim, chunk, data);
        machines::chunk_data_save(dim, chunk, data);
        pipes::chunk_data_save(dim, chunk, data);
    }

    /// Prepare pipes, boilers, valves and tanks before the shared transport walk.
    /// Pumps, drains and spouts run on their registration turn in that walk.
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        lava::tick(lv);
        open_end::tick(lv);
        place::tick_layers(lv);
        pipes::tick_boilers(lv);
        plumbing::tick(lv);
        rims::tick(lv);
        engine::tick(lv);
    }

    fn client_reset(&self) {
        client::reset();
    }

    /// The time clients' pipe flows are measured by (`flows`).
    fn client_tick(&self, host: &mut dyn mod_api::ClientHost) {
        flows::client_tick(&*host);
    }

    /// Honey and chocolate meeting lava (`AllFluids.registerFluidInteractions`), looked at when
    /// either ticks (`lava`).
    fn fluid_tick(&self, level: &dyn mod_api::LevelHost, event: &mut mod_api::FluidTickEvent<'_>) {
        lava::fluid_tick(level, event);
    }

    /// The pipes beside a Create block that changed draw their pieces again (`rims`).
    fn block_changed(&self, dim: usize, pos: [i32; 3], _old: u32, new: u32) {
        flows::forget(dim, pos);
        engine::changed(dim, pos, new);
        rims::changed(dim, pos);
    }

    fn write_entities(&self, lv: &mut ServerLevel<'_>) {
        flows::send(lv);
    }

    fn world_closed(&self) {
        lava::reset();
        open_end::reset();
        flows::reset();
        place::reset();
        rims::reset();
        drawn::reset();
        engine::reset();
    }

    /// What clients see of tanks, spouts, drains, boilers and hoses, sent with their chunks too.
    fn synced_keys(&self) -> &'static [&'static str] {
        &[
            TANK,
            TANK_FLUIDS,
            SPOUT_START,
            HOSE,
            BOILER,
            rims::RIMS,
            drawn::DRAIN_ITEM,
            engine::ENGINE_SPEED,
            engine::STEAM,
            plumbing::FILTER,
            plumbing::WHISTLE,
            flows::FLOW,
            flows::FLOW_FLUIDS,
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

    /// The wrench on a tank, a pipe or a whistle, not sneaking (`wrench_pipe` / `wrench_whistle`,
    /// after a bracket came off: `decor`'s part).
    fn wrench_used(
        &self,
        lv: &mut ServerLevel<'_>,
        player: mod_api::PlayerRef,
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
        // `FluidPipeBlock.onWrenched`: a pipe made glass while something flows in it.
        let flowing = block == Block::FluidPipe && flows::has_flow(lv.dim, pos);
        let done = plumbing::wrench_pipe(lv, pos, block, clicked, point);
        if flowing && lv.block_name(pos).as_deref() == Some("create:glass_fluid_pipe") {
            crate::glue::advancements::award(lv.host, player, "glass_pipe");
        }
        done || plumbing::wrench_whistle(lv, pos, block)
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

/// The tanks whose contents changed this tick, to their comparators and clients: after redstone's
/// `Detectors`, which read the changes, and before the solve.
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

/// What a block entity shows under a key: numbers, and the keys of the fluids they number.
type Shown = (Vec<i32>, Vec<String>);

thread_local! {
    /// What each spout's, drain's and boiler's block entity last showed its clients, by level
    /// and position.
    static SHOWN: RefCell<HashMap<ShownKey, Shown>> =
        RefCell::new(HashMap::new());
}

/// Spouts' and item drains' contents (fluid, amount, capacity, under `TANK`) and boilers'
/// states (`BOILER`) to their block entities' clients when they changed (`notifyUpdate`).
fn send_shown(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let time = lv.game_time();
    let now: Vec<([i32; 3], &'static str, Shown)> = with_world(|w| {
        let d = &w.dims[dim];
        let fluid = |f: Option<(Fluid, u32)>, capacity: u32| {
            let mut codes = Codes::default();
            let v = match f {
                Some((f, n)) => vec![codes.code(Some(f)), n as i32, capacity as i32],
                None => vec![255, 0, capacity as i32],
            };
            (v, codes.keys)
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
            (*p, BOILER, (v, Vec::new()))
        });
        // A spout working shows when its cycle started (`processingTicks`).
        let working = d
            .spouts
            .iter()
            .filter(|(_, s)| s.processing >= 0)
            .map(|(p, s)| {
                let start = time - i64::from(create_engine::spout::FILLING_TIME - s.processing);
                (*p, SPOUT_START, (vec![start as i32], Vec::new()))
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
                && let Some(width) = v.0.pop()
            {
                v.0.push(boiler_occlusion(lv, p, width));
            }
            (p, k, v)
        })
        .collect();
    let changed: Vec<_> = SHOWN.with_borrow_mut(|shown| {
        // Gone (a boiler no more, a drain broken): shown as nothing.
        let mut changed: Vec<_> = shown
            .keys()
            .filter(|(d, p, k)| *d == dim && !now.iter().any(|(q, j, _)| q == p && j == k))
            .map(|(_, p, k)| (*p, *k, Shown::default()))
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
    for (pos, key, (v, keys)) in changed {
        // A spout's or drain's contents are saved with its column.
        if key == TANK {
            with_world(|w| w.mark_unsaved(dim, pos));
        }
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let mut data = Compound::from([(key.into(), Tag::IntArray(v))]);
        if key == TANK {
            // The potion's key, or none.
            let keys = keys
                .iter()
                .map(|k| Tag::String(k.as_str().into()))
                .collect();
            data.insert(TANK_FLUIDS.into(), Tag::List(keys));
        }
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

/// FluidTankBlockEntity.read for column members; accept both older controller-only formats.
fn tanks_from_nbt(data: &Compound) -> Vec<([i32; 3], create_engine::tank::SavedTank)> {
    if let Some(Tag::IntArray(v)) = data.get(&"tank_entities".into()) {
        let keys = fluid_keys(data, "tank_entities");
        return v
            .as_chunks::<14>()
            .0
            .iter()
            .map(|c| {
                let p = [c[0], c[1], c[2]];
                (
                    p,
                    create_engine::tank::SavedTank {
                        part: create_engine::tank::Part {
                            controller: [c[8], c[9], c[10]],
                            window: c[11] != 0,
                            update_connectivity: c[12] != 0,
                        },
                        multi: (c[13] != 0).then_some(Multi {
                            tank: Tank {
                                fluid: Codes::fluid(&keys, c[3]),
                                amount: c[4].max(0) as u32,
                            },
                            width: c[5] as u8,
                            height: c[6] as u8,
                            window: c[7] != 0,
                        }),
                    },
                )
            })
            .collect();
    }
    let keys = fluid_keys(data, "tank_multis");
    let tank = |c: &[i32]| Tank {
        fluid: Codes::fluid(&keys, c[3]),
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
    out.into_iter()
        .map(|(p, m)| (p, create_engine::tank::SavedTank::controller(p, m)))
        .collect()
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
    let mut codes = Codes::default();
    let v = m.map_or_else(Vec::new, |m| {
        vec![
            codes.code(m.tank.fluid),
            m.tank.amount as i32,
            i32::from(m.width),
            i32::from(m.height),
            i32::from(m.window),
        ]
    });
    let mut data = Compound::from([(TANK.into(), Tag::IntArray(v))]);
    // Always sent, so that a tank's potion key is replaced with its fluid.
    data.insert(
        TANK_FLUIDS.into(),
        Tag::List(
            codes
                .keys
                .iter()
                .map(|k| Tag::String(k.as_str().into()))
                .collect(),
        ),
    );
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
            fluid: Codes::fluid(&fluid_keys(data, TANK), fluid),
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

/// Deliver committed shape updates before the level starts scheduled ticks.
pub(crate) fn tick_pipe_shapes(lv: &mut ServerLevel<'_>) {
    pipes::tick_pipe_shapes(lv);
}
