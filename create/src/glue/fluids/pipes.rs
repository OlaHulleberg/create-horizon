//! Pipes and pumps (`FluidPropagator`, `FluidNetwork`), boilers and steam engines, and tank
//! blocks' states.

use super::super::processing;
use super::super::server::{ServerLevel, UPDATE_CLIENTS};
use super::super::world::with_world;
use super::{all, flag, info_at, plumbing, replaceable, state_with};
use crate::glue::kinetics::dir_name;
use create_engine::Block;
use create_engine::fluids::{self, Cell};
use create_engine::kinetics::Dir;
use create_engine::open_end::OpenEnd;
use create_engine::tank::{Codes, Fluid};
use mod_api::{BlockState, Compound, HostBlockStates, Tag};

/// What a block is to the pipes (`FluidPropagator`): a pipe and its open sides, a pump and its
/// facing, a tank, open space a pipe may end in, or neither.
pub(super) fn pipe_cell(lv: &ServerLevel<'_>, p: [i32; 3]) -> Cell {
    pipe_cell_toward(lv, p, None)
}

/// [`pipe_cell`] of the block at `p` entered by way `toward` from a pipe: open space is an open
/// end only as `FluidPropagator.isOpenEnd` says, by its face toward the pipe.
pub(super) fn pipe_cell_toward(lv: &ServerLevel<'_>, p: [i32; 3], toward: Option<Dir>) -> Cell {
    if !lv.is_loaded(p) {
        return Cell::Solid;
    }
    if let Some((block, info)) = info_at(lv, p) {
        return match block {
            Block::FluidPipe => Cell::Pipe(pipe_sides_at(lv, p)),
            Block::MechanicalPump => info.facing.map_or(Cell::Solid, Cell::Pump),
            Block::FluidTank | Block::Spout | Block::ItemDrain | Block::Basin => Cell::Tank,
            b => plumbing::plumbing_cell(lv, b, &info, p).unwrap_or(Cell::Solid),
        };
    }
    if lv.block_state(p).is_some_and(|state| {
        fluids::vanilla_target(
            &state.block().id(&*lv.host),
            super::property(lv, p, "honey_level").is_some(),
        )
    }) {
        Cell::VanillaTarget
    } else if open_end(lv, p, toward) {
        Cell::Open
    } else {
        Cell::Solid
    }
}

/// `FluidPropagator.isOpenEnd` of a block that is no pipe, pump or Create fluid handler, entered
/// by way `toward`: a beehive or cauldron (`canProvideFluidWithoutCapability`); not a block with
/// a sturdy face toward the pipe (but what fans blow through) or another fluid handler; and
/// only what can be replaced (and broken) or waterlogged.
fn open_end(lv: &ServerLevel<'_>, p: [i32; 3], toward: Option<Dir>) -> bool {
    let Some(state) = lv.block_state(p) else {
        return false;
    };
    let name = state.block().id(&*lv.host);
    if fluids::vanilla_target(&name, super::property(lv, p, "honey_level").is_some()) {
        return true;
    }
    if let Some(d) = toward {
        use mod_api::BlockFace;
        let face = match d.opposite() {
            Dir::Down => BlockFace::Down,
            Dir::Up => BlockFace::Up,
            Dir::North => BlockFace::North,
            Dir::South => BlockFace::South,
            Dir::West => BlockFace::West,
            Dir::East => BlockFace::East,
        };
        let sturdy = lv
            .level
            .block_face_sturdy(&*lv.host, p, face, mod_api::SupportType::Full)
            == Some(true);
        if sturdy
            && lv
                .host
                .block_in_tag(state.block(), "create:fan_transparent")
                != Some(true)
        {
            return false;
        }
    }
    let unbreakable = lv
        .host
        .block_states()
        .destroy_speed(state)
        .is_some_and(|s| s == -1.0);
    (replaceable(lv, p) && !unbreakable) || super::property(lv, p, "waterlogged").is_some()
}

/// A pipe state's open sides (`Dir::bit`).
pub(super) fn pipe_sides_of(lv: &ServerLevel<'_>, state: BlockState) -> u8 {
    let meta = lv.host.block_states();
    Dir::ALL
        .into_iter()
        .filter(|d| state.property(&meta, dir_name(*d)).ok().flatten() == Some("true"))
        .fold(0, |m, d| m | d.bit())
}

pub(super) fn pipe_sides_at(lv: &ServerLevel<'_>, p: [i32; 3]) -> u8 {
    lv.block_state(p).map_or(0, |s| pipe_sides_of(lv, s))
}

/// `FluidPipeBlock.updateBlockState` for the pipe at `pos`, written when it changes.
pub(super) fn connect_pipe(lv: &mut ServerLevel<'_>, pos: [i32; 3], preferred: Option<Dir>) {
    if info_at(lv, pos).map(|(b, _)| b) != Some(Block::FluidPipe) {
        return;
    }
    let Some(state) = lv.block_state(pos) else {
        return;
    };
    let previous = pipe_sides_of(lv, state);
    let sides = fluids::pipe_sides(
        previous,
        |d| pipe_cell(lv, super::offset(pos, d)),
        preferred.unwrap_or(Dir::North),
    );
    if sides == previous && preferred.is_none() {
        return;
    }
    let meta = lv.host.block_states();
    let mut next = state;
    for d in Dir::ALL {
        if let Ok(s) = next.with(&meta, dir_name(d), flag(sides & d.bit() != 0)) {
            next = s;
        }
    }
    if next != state {
        lv.set_state(pos, next, all());
    }
}

/// Pipes and pumps: pipes that changed connect again; turning pumps find their ends when pipes
/// changed (`distributePressureTo`) and carry fluid from the first end behind them to the
/// first end in front that takes it, `max(1, pressure / 2)` mB a tick (`FluidNetwork`).
pub(crate) fn tick_pipes(lv: &mut ServerLevel<'_>) -> bool {
    let dim = lv.dim;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].pipe_checks)).unwrap_or_default();
    checks.sort();
    checks.dedup();
    for pos in checks {
        connect_pipe(lv, pos, None);
    }
    with_world(|w| std::mem::replace(&mut w.dims[dim].pipes_changed, false)).unwrap_or(false)
}

/// Publish flows after all pumps have had their registration turn.
pub(crate) fn finish_ticks(lv: &mut ServerLevel<'_>) {
    let pumps = with_world(|w| w.dims[lv.dim].pumping.clone()).unwrap_or_default();
    super::flows::keep_only(lv.dim, &pumps);
    collide(lv);
    super::flows::send(lv);
    super::drawn::send(lv);
}

/// `FluidReactions.handlePipeFlowCollision`: a pipe where flows of two fluids come in breaks
/// (`BlockHelper.destroyBlock`, its drops and particles) and becomes cobblestone (water and
/// lava), limestone or scoria (lava and honey or chocolate), or stays broken.
fn collide(lv: &mut ServerLevel<'_>) {
    let now = lv.game_time();
    for (pos, a, b) in super::flows::collisions(lv.dim, now) {
        crate::glue::advancements::award_placer(lv, pos, "cross_streams");
        lv.destroy_block(pos, true);
        if let Some(state) =
            create_engine::open_end::flow_collision(a, b).and_then(|n| state_with(lv, n, &[]))
        {
            lv.set_state(pos, state, all());
        }
    }
}

pub(crate) fn tick_pump(lv: &mut ServerLevel<'_>, pos: [i32; 3], changed: bool) {
    if with_world(|w| w.dims[lv.dim].pumping.contains(&pos)) == Some(true) {
        let changed = changed || with_world(|w| w.dims[lv.dim].pipes_changed).unwrap_or(false);
        pump(lv, pos, changed);
    }
}

fn pump(lv: &mut ServerLevel<'_>, pos: [i32; 3], changed: bool) {
    let dim = lv.dim;
    let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
    let Some(facing) = info_at(lv, pos).and_then(|(_, i)| i.facing) else {
        return;
    };
    let Some(mut pump) = with_world(|w| w.dims[dim].pumps.get(&pos).cloned()).flatten() else {
        return;
    };
    if changed || !pump.found {
        let cell = |p, d| pipe_cell_toward(lv, p, Some(d));
        let pull = fluids::reach_toward(pos, facing.opposite(), speed, true, cell);
        let push = fluids::reach_toward(pos, facing, speed, false, cell);
        let ways_pull = fluids::ways_toward(pos, facing.opposite(), true, cell);
        let ways_push = fluids::ways_toward(pos, facing, false, cell);
        // Pipes changed elsewhere leave this pump's network as it was.
        let same = pump.found
            && pull == pump.pull
            && push == pump.push
            && ways_pull == pump.ways_pull
            && ways_push == pump.ways_push;
        if !pump.found {
            let way = ways_pull.iter().chain(&ways_push);
            let pipes = way.flat_map(|(_, w)| w.iter().map(|s| s.pos));
            super::flows::found(dim, pipes.chain([pos]));
        }
        if !same {
            (pump.pull, pump.push) = (pull, push);
            (pump.ways_pull, pump.ways_push) = (ways_pull, ways_push);
            // `wipePressure`, `FluidNetwork.reset`: the flows start again from the pump.
            pump.pumped = 0;
            super::flows::clear(dim, pos);
        }
        pump.found = true;
    }
    // Only terminal sources participate in the current endpoint transfer model.
    // A pump is a pressure target, but FlowSource.OtherPipe.isEndpoint is false.
    // FluidNetwork's connection/flow traversal through it is still required.
    let from = pump
        .pull
        .iter()
        .find(|e| !e.blocked && e.kind != fluids::Endpoint::Pump)
        .copied();
    let mut flowing = false;
    if let (Some(from), false) = (from, pump.push.is_empty()) {
        let amount = fluids::transfer_speed(from.pressure);
        // What the source can give this tick: a creative tank or a hose pulley's, else...
        let special = if from.kind == fluids::Endpoint::Handler {
            plumbing::source(lv, from.pos, amount)
        } else {
            None
        };
        let drained_from_drain = from.kind == fluids::Endpoint::Handler
            && with_world(|w| w.dims[dim].drains.contains_key(&from.pos)).unwrap_or(false);
        let from_basin = from.kind == fluids::Endpoint::Handler
            && info_at(lv, from.pos).map(|(b, _)| b) == Some(Block::Basin);
        let offer = if let Some(offer) = special {
            offer
        } else if from_basin {
            // A basin gives from its outputs.
            let mut t = processing::basin_tanks(dim, from.pos);
            create_engine::basin::drain(&mut t.output, amount)
        } else if drained_from_drain {
            with_world(|w| {
                w.dims[dim]
                    .drains
                    .get(&from.pos)
                    .and_then(|d| d.fluid.map(|(f, n)| (f, n.min(amount))))
            })
            .flatten()
        } else if from.kind == fluids::Endpoint::Handler {
            with_world(|w| {
                // A boiler gives nothing (`BoilerFluidHandler.drain`).
                if w.is_boiler(dim, from.pos) {
                    return None;
                }
                w.tank(dim, from.pos)
                    .and_then(|(_, m)| m.tank.fluid.map(|f| (f, m.tank.amount.min(amount))))
            })
            .flatten()
        } else {
            // An open end: its own tank, else the source in front (`OpenEndFluidHandler.drain`,
            // simulated).
            with_open_end(lv, &from, |end, front| {
                end.drain(front, amount, None, false)
            })
        };
        // Smart pipes on the way let only their filter's fluid through.
        let offer = offer.filter(|(f, _)| from.passes(*f));
        if let Some((fluid, _)) = offer {
            flowing = true;
            // Flows of another fluid, or none yet: they start from the source again.
            if super::flows::running(dim, pos) != Some(fluid) {
                pump.pumped = 0;
                let pull = pump
                    .ways_pull
                    .iter()
                    .find(|(end, _)| *end == from.pos)
                    .map_or(&[][..], |(_, w)| w.as_slice());
                let push: Vec<(fluids::End, Vec<fluids::Step>)> = pump
                    .push
                    .iter()
                    .filter_map(|e| {
                        let way = pump.ways_push.iter().find(|(end, _)| *end == e.pos)?;
                        Some((*e, way.1.clone()))
                    })
                    .collect();
                let flows = fluids::flows(pos, facing, speed, &from, pull, &push, fluid);
                super::flows::start(dim, pos, fluid, flows, lv.game_time());
            }
        }
        if let Some((fluid, n)) = offer {
            // The flows on the way advance (`manageFlows`); the ends in front they have
            // reached take an equal share each (`FluidNetwork.tick`'s `availableOutputs`).
            let pumped = pump.advance_flow();
            // No list of them is made: the k-th reached end is found when it is filled.
            let push = std::mem::take(&mut pump.push);
            let reaches = |to: &&fluids::End| {
                to.kind != fluids::Endpoint::Pump
                    && to.passes(fluid)
                    && pumped >= fluids::arrival_ticks(&from, to, speed)
            };
            let count = push.iter().filter(reaches).count();
            let moved = fluids::share(count, n, |k, give| {
                let Some(to) = push.iter().filter(reaches).nth(k) else {
                    return 0;
                };
                fill_end(lv, to, fluid, give)
            });
            if moved > 0 {
                if special.is_some() {
                    plumbing::take(lv, from.pos, moved);
                } else if from_basin {
                    let mut t = processing::basin_tanks(dim, from.pos);
                    create_engine::basin::drain(&mut t.output, moved);
                    with_world(|w| w.set_basin_fluids(dim, from.pos, t));
                } else if drained_from_drain {
                    with_world(|w| {
                        if let Some(d) = w.dims[dim].drains.get_mut(&from.pos) {
                            create_engine::drain::drain(d, moved);
                            w.dims[dim].analog_checks.push(from.pos);
                        }
                    });
                } else if from.kind == fluids::Endpoint::Handler {
                    with_world(|w| w.drain_tank(dim, from.pos, moved));
                } else {
                    with_open_end(lv, &from, |end, front| {
                        end.drain(front, moved, Some(fluid), true)
                    });
                }
                with_world(|w| w.mark_unsaved(dim, pos));
            }
            pump.push = push;
        }
    }
    // The source gives no more: the flows recede from it, and start again from nothing.
    if !flowing {
        pump.pumped = 0;
        super::flows::stop(dim, pos, lv.game_time());
    }
    with_world(|w| {
        w.dims[dim].pumps.insert(pos, pump);
    });
}

/// Up to `n` mB of `fluid` into end `to` of `pump`'s pipes in front: a hose pulley's or creative
/// tank's sink, a basin's inputs, a boiler's supply, a spout's tank, a tank's, or an open end's
/// space (a source placed per bucket, `provideFluidToSpace`). How much it took.
fn fill_end(lv: &mut ServerLevel<'_>, to: &fluids::End, fluid: Fluid, n: u32) -> u32 {
    let dim = lv.dim;
    if to.kind == fluids::Endpoint::Handler {
        if let Some(took) = plumbing::sink(lv, to.pos, fluid, n) {
            return took;
        }
        if info_at(lv, to.pos).map(|(b, _)| b) == Some(Block::Basin) {
            // A basin takes into its inputs.
            let mut t = processing::basin_tanks(dim, to.pos);
            let took = create_engine::basin::fill(&mut t.input, fluid, n);
            if took > 0 {
                with_world(|w| w.set_basin_fluids(dim, to.pos, t));
            }
            return took;
        }
        return with_world(|w| {
            // A boiler takes all the water as supply, none into its tank
            // (`BoilerFluidHandler.fill`).
            if let Some(took) = w.fill_boiler(dim, to.pos, fluid, n) {
                return took;
            }
            // A tank block fills its tank (by its controller).
            match w.dims[dim].spouts.get_mut(&to.pos) {
                Some(s) => {
                    let took = create_engine::spout::fill(s, fluid, n);
                    if took > 0 {
                        w.dims[dim].analog_checks.push(to.pos);
                    }
                    took
                }
                None => w.fill_tank(dim, to.pos, fluid, n),
            }
        })
        .unwrap_or(0);
    }
    // An open end: `OpenEndFluidHandler.fill`.
    with_open_end(lv, to, |end, front| end.fill(front, fluid, n, true))
}

/// `f` on the open end `end` of a pump's pipes (its own tank, kept by the pipe or pump it
/// belongs to and the side it is open to) and the space in front of it; the tank is kept, and
/// its column saved, when it changed.
fn with_open_end<R>(
    lv: &mut ServerLevel<'_>,
    end: &fluids::End,
    f: impl FnOnce(&mut OpenEnd, &mut super::open_end::Front<'_, '_>) -> R,
) -> R {
    let dim = lv.dim;
    let owner = super::offset(end.pos, end.face.opposite());
    let key = (owner, end.face);
    let before = with_world(|w| w.dims[dim].open_ends.get(&key).copied())
        .flatten()
        .unwrap_or_default();
    let mut open = before;
    let out = f(
        &mut open,
        &mut super::open_end::Front {
            lv,
            pos: end.pos,
            down: end.face == Dir::Down,
            pipe: owner,
        },
    );
    if open != before {
        with_world(|w| {
            if open == OpenEnd::default() {
                w.dims[dim].open_ends.remove(&key);
            } else {
                w.dims[dim].open_ends.insert(key, open);
            }
            w.mark_unsaved(dim, owner);
        });
    }
    out
}

/// The key of the open ends' tanks in a column's data: for each, the pipe's position, the side
/// (`Dir::ALL` index), its fluid (`tank::Codes`, a potion's key beside it), amount and whether
/// it was pulled from (`OpenEndedPipe`'s `Pulling`).
const OPEN_ENDS: &str = "open_ends";

pub(super) fn chunk_data_save(dim: usize, chunk: [i32; 2], data: &mut Compound) {
    let here = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]);
    let Some(mut ends) = with_world(|w| {
        w.dims[dim]
            .open_ends
            .iter()
            .filter(|((p, _), _)| here(p))
            .map(|(k, e)| (*k, *e))
            .collect::<Vec<_>>()
    }) else {
        return;
    };
    if ends.is_empty() {
        return;
    }
    ends.sort_by_key(|((p, d), _)| (*p, Dir::ALL.iter().position(|x| x == d)));
    let mut codes = Codes::default();
    let v: Vec<i32> = ends
        .iter()
        .flat_map(|((p, d), e)| {
            let side = Dir::ALL.iter().position(|x| x == d).unwrap_or(0) as i32;
            [
                p[0],
                p[1],
                p[2],
                side,
                codes.code(e.tank.map(|t| t.0)),
                e.tank.map_or(0, |t| t.1 as i32),
                i32::from(e.pulling),
            ]
        })
        .collect();
    data.insert(OPEN_ENDS.into(), Tag::IntArray(v));
    super::put_fluid_keys(data, OPEN_ENDS, codes);
}

pub(super) fn chunk_load(dim: usize, data: &Compound) {
    let Some(Tag::IntArray(v)) = data.get(&OPEN_ENDS.into()) else {
        return;
    };
    let keys = super::fluid_keys(data, OPEN_ENDS);
    let ends: Vec<_> = v
        .as_chunks::<7>()
        .0
        .iter()
        .filter_map(|c| {
            let side = *Dir::ALL.get(c[3].clamp(0, 5) as usize)?;
            let tank = Codes::fluid(&keys, c[4]).map(|f| (f, c[5].max(0) as u32));
            let end = OpenEnd {
                tank: tank.filter(|t| t.1 > 0),
                pulling: c[6] != 0,
            };
            Some((([c[0], c[1], c[2]], side), end))
        })
        .collect();
    with_world(|w| w.dims[dim].open_ends.extend(ends));
}

/// Boilers and steam engines (`BoilerData.tick`, `SteamEngineBlockEntity.tick`): tanks whose
/// engines changed count them again, shafts in front of engines become powered shafts or plain
/// ones again, boilers whose heaters changed read them, and boilers sample their water supply.
/// Nothing runs without a boiler or a changed engine.
pub(super) fn tick_boilers(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let changes = with_world(|w| {
        w.evaluate_boilers(dim);
        w.shaft_changes(dim)
    })
    .unwrap_or_default();
    for (pos, powered) in changes {
        // `PoweredShaftBlock.getEquivalent` and its way back: axis and waterlogging kept.
        let axis = super::property(lv, pos, "axis").unwrap_or_default();
        let water = super::property(lv, pos, "waterlogged").unwrap_or_default();
        let name = if powered {
            "create:powered_shaft"
        } else {
            "create:shaft"
        };
        if let Some(state) = state_with(lv, name, &[("axis", &axis), ("waterlogged", &water)]) {
            lv.set_state(pos, state, all());
        }
    }
    for (tank, width) in with_world(|w| w.take_heat_checks(dim)).unwrap_or_default() {
        // `updateTemperature`: every block under the tank's bottom layer.
        let w = i32::from(width);
        let heats: Vec<i32> = (0..w)
            .flat_map(|x| (0..w).map(move |z| (x, z)))
            .map(|(x, z)| boiler_heater(lv, [tank[0] + x, tank[1] - 1, tank[2] + z]))
            .collect();
        with_world(|w| w.set_boiler_heat(dim, tank, &heats));
    }
    with_world(|w| w.tick_boilers(dim));
    crate::glue::advancements::boilers(lv);
}

/// `BoilerHeater.findHeat` under a boiler: a blaze burner by its heat (`BoilerHeaters
/// .blazeBurner`), another block in `create:passive_boiler_heaters` passive unless it is out
/// (`BoilerHeaters.passive`: an unlit campfire), anything else none.
fn boiler_heater(lv: &ServerLevel<'_>, p: [i32; 3]) -> i32 {
    use create_engine::steam;
    if let Some((Block::BlazeBurner, info)) = info_at(lv, p) {
        return info.heat.map_or(steam::NO_HEAT, steam::blaze_burner_heat);
    }
    let Some(state) = lv.block_state(p) else {
        return steam::NO_HEAT;
    };
    if lv
        .host
        .block_in_tag(state.block(), "create:passive_boiler_heaters")
        != Some(true)
    {
        return steam::NO_HEAT;
    }
    let lit = super::property(lv, p, "lit");
    steam::passive_heat(lit.as_deref() != Some("false"))
}

/// Tank blocks whose tank formed, split or became a boiler take its `top`, `bottom` and window
/// `shape` (`notifyMultiUpdated`, `setWindows`, `removeController`), with clients told and no
/// neighbour updates as Create sets them.
pub(super) fn tick_tank_states(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    for pos in with_world(|w| w.take_tank_states(dim)).unwrap_or_default() {
        if !lv.is_loaded(pos) {
            continue;
        }
        let Some(want) = with_world(|w| w.tank_shape(dim, pos)).flatten() else {
            continue;
        };
        if !matches!(
            info_at(lv, pos).map(|(b, _)| b),
            Some(Block::FluidTank | Block::CreativeFluidTank)
        ) {
            continue;
        }
        let Some(state) = lv.block_state(pos) else {
            continue;
        };
        let meta = lv.host.block_states();
        let next = state
            .with(&meta, "top", flag(want.top))
            .and_then(|s| s.with(&meta, "bottom", flag(want.bottom)))
            .and_then(|s| s.with(&meta, "shape", want.shape.name()));
        if let Ok(next) = next
            && next != state
        {
            lv.set_state(pos, next, UPDATE_CLIENTS);
        }
    }
}
