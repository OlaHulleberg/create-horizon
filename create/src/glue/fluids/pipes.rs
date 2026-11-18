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
use create_engine::tank::Fluid;
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

/// FluidTransportBehaviour.cacheFlows/loadFlows around a pipe representation change.
pub(super) fn replace_pipe_state(lv: &mut ServerLevel<'_>, pos: [i32; 3], state: BlockState) {
    let dim = lv.dim;
    let previous = with_world(|w| w.dims[dim].fluid_transport.pipes.remove(&pos)).flatten();
    lv.set_state(pos, state, all());
    if let Some(previous) = previous {
        with_world(|w| w.dims[dim].fluid_transport.pipes.insert(pos, previous));
    }
}

/// Apply queued updateShape notifications from committed world changes.
pub(crate) fn tick_pipe_shapes(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].pipe_checks)).unwrap_or_default();
    checks.sort();
    checks.dedup();
    for p in checks {
        connect_pipe(lv, p, None);
    }
}

/// Initialize behaviours and process due invalidations before the shared walk.
pub(crate) fn tick_pipes(lv: &mut ServerLevel<'_>) {
    tick_pipe_shapes(lv);
    let dim = lv.dim;
    let positions = with_world(|w| w.fluid_transport_positions(dim)).unwrap_or_default();
    let mut transport =
        with_world(|w| std::mem::take(&mut w.dims[dim].fluid_transport)).unwrap_or_default();
    for p in positions {
        transport.register(
            p,
            pipe_cell(lv, p),
            with_world(|w| w.speed(dim, p)).flatten().unwrap_or(0.),
        );
    }
    transport.prepare(&TransportHost(lv));
    put_transport(dim, transport);
}

/// FluidTransportBehaviour.tick on this block entity's registration turn.
/// For valves this precedes the pointer's tick, as SmartBlockEntity.tick does.
pub(crate) fn tick_transport_pipe(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    if with_world(|w| w.dims[dim].fluid_transport.pipes.contains_key(&pos)) != Some(true) {
        return;
    }
    with_world(|w| w.fluid_transport_base_tick(dim, pos));
    let mut transport =
        with_world(|w| std::mem::take(&mut w.dims[dim].fluid_transport)).unwrap_or_default();
    if let Some((a, b)) = transport.tick_pipe(&mut TransportHost(lv), pos) {
        crate::glue::advancements::award_placer(lv, pos, "cross_streams");
        lv.destroy_block(pos, true);
        transport.remove(pos);
        if let Some(state) =
            create_engine::open_end::flow_collision(a, b).and_then(|name| state_with(lv, name, &[]))
        {
            lv.set_state(pos, state, all());
        }
    }
    put_transport(dim, transport);
}
fn put_transport(dim: usize, mut transport: create_engine::fluids::runtime::Transport) {
    with_world(|w| {
        transport
            .changed
            .append(&mut w.dims[dim].fluid_transport.changed);
        w.dims[dim].fluid_transport = transport;
    });
}
pub(crate) fn finish_ticks(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    with_world(|w| {
        let positions = w.dims[dim]
            .fluid_transport
            .pipes
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for p in positions {
            w.mark_unsaved(dim, p);
        }
    });
    super::flows::send(lv);
    super::drawn::send(lv);
}

struct TransportHost<'a, 'b>(&'a mut ServerLevel<'b>);
use create_engine::fluids::{
    runtime::{Face, Host},
    transport::Source,
};
impl Host for TransportHost<'_, '_> {
    fn time(&self) -> i64 {
        self.0.game_time()
    }
    fn cell(&self, p: [i32; 3], d: Dir) -> Cell {
        pipe_cell_toward(self.0, p, Some(d))
    }
    fn loaded(&self, p: [i32; 3]) -> bool {
        self.0.is_loaded(p)
    }
    fn speed(&self, p: [i32; 3]) -> f32 {
        with_world(|w| w.speed(self.0.dim, p))
            .flatten()
            .unwrap_or(0.)
    }
    fn notifies_pipe_neighbours(&self, pos: [i32; 3]) -> bool {
        create_engine::fluids::notifies_pipe_neighbours(
            info_at(self.0, pos).map(|(b, _)| b),
            self.0
                .block_name(pos)
                .and_then(|s| Fluid::of_id(&s))
                .is_some_and(|f| f.placeable()),
        )
    }

    fn tank_fluids(&self, face: Face) -> Vec<Fluid> {
        if !self.0.is_loaded(face.connected()) {
            return Vec::new();
        }
        super::fluid_handler_tanks(self.0, face.connected())
            .unwrap_or_default()
            .into_iter()
            .filter(|slot| slot.amount > 0)
            .filter_map(|slot| slot.fluid)
            .collect()
    }
    fn offer(
        &mut self,
        face: Face,
        source: &mut Source,
        n: u32,
        fluid: Option<Fluid>,
    ) -> Option<(Fluid, u32)> {
        let lv = &mut *self.0;
        let dim = lv.dim;
        let pos = face.connected();
        if !lv.is_loaded(pos) {
            return None;
        }
        if let Source::Open(open) = source {
            return open.drain(
                &mut super::open_end::Front {
                    lv,
                    pos,
                    down: face.side == Dir::Down,
                    pipe: face.pos,
                },
                n,
                fluid,
                false,
            );
        }
        if !matches!(source, Source::Handler) {
            return None;
        }
        let offer = if let Some(offer) = plumbing::source(lv, pos, n, fluid) {
            offer
        } else if info_at(lv, pos).map(|(b, _)| b) == Some(Block::Basin) {
            let mut t = processing::basin_tanks(dim, pos);
            create_engine::basin::drain_matching(&mut t.output, n, fluid)
        } else {
            with_world(|w| {
                if w.is_boiler(dim, pos) {
                    return None;
                }
                if let Some(d) = w.dims[dim].drains.get(&pos) {
                    return d.fluid.map(|(f, k)| (f, k.min(n)));
                }
                w.tank(dim, pos)
                    .and_then(|(_, m)| m.tank.fluid.map(|f| (f, m.tank.amount.min(n))))
            })
            .flatten()
        };
        offer.filter(|(f, _)| fluid.is_none_or(|wanted| wanted == *f))
    }
    fn drain(&mut self, face: Face, source: &mut Source, fluid: Fluid, n: u32) -> u32 {
        let lv = &mut *self.0;
        let dim = lv.dim;
        let pos = face.connected();
        if !lv.is_loaded(pos) {
            return 0;
        }
        if let Source::Open(open) = source {
            return open
                .drain(
                    &mut super::open_end::Front {
                        lv,
                        pos,
                        down: face.side == Dir::Down,
                        pipe: face.pos,
                    },
                    n,
                    Some(fluid),
                    true,
                )
                .map_or(0, |(_, n)| n);
        }
        if plumbing::source(lv, pos, n, Some(fluid)).is_some() {
            return plumbing::take(lv, pos, n, fluid);
        }
        if info_at(lv, pos).map(|(b, _)| b) == Some(Block::Basin) {
            let mut t = processing::basin_tanks(dim, pos);
            let took = create_engine::basin::drain_matching(&mut t.output, n, Some(fluid))
                .map_or(0, |(_, n)| n);
            with_world(|w| w.set_basin_fluids(dim, pos, t));
            return took;
        }
        with_world(|w| {
            if let Some(d) = w.dims[dim].drains.get_mut(&pos) {
                let took = create_engine::drain::drain(d, n).map_or(0, |(_, n)| n);
                w.dims[dim].analog_checks.push(pos);
                took
            } else {
                w.drain_tank(dim, pos, n).map_or(0, |(_, n)| n)
            }
        })
        .unwrap_or(0)
    }
    fn fill(
        &mut self,
        face: Face,
        source: &mut Source,
        fluid: Fluid,
        n: u32,
        execute: bool,
    ) -> u32 {
        let lv = &mut *self.0;
        let pos = face.connected();
        let dim = lv.dim;
        if let Source::Open(open) = source {
            return open.fill(
                &mut super::open_end::Front {
                    lv,
                    pos,
                    down: face.side == Dir::Down,
                    pipe: face.pos,
                },
                fluid,
                n,
                execute,
            );
        }
        if execute {
            return fill_end(lv, pos, fluid, n);
        }
        if let Some(took) = plumbing::simulate_sink(lv, pos, fluid, n) {
            return took;
        }
        if info_at(lv, pos).map(|(b, _)| b) == Some(Block::Basin) {
            let mut t = processing::basin_tanks(dim, pos);
            return create_engine::basin::fill(&mut t.input, fluid, n);
        }
        with_world(|w| {
            if w.dims[dim].plumbing.creative.contains_key(&pos) {
                return n;
            }
            if let Some(took) = w.simulate_fill_boiler(dim, pos, fluid, n) {
                return took;
            }
            if let Some(s) = w.dims[dim].spouts.get(&pos) {
                return create_engine::spout::fill(&mut s.clone(), fluid, n);
            }
            w.tank(dim, pos).map_or(0, |(_, m)| {
                m.tank.clone().fill_amount(fluid, n, m.capacity())
            })
        })
        .unwrap_or(0)
    }
    fn identity(&self, face: Face) -> [i32; 3] {
        with_world(|w| w.tank(self.0.dim, face.connected()).map(|(p, _)| p))
            .flatten()
            .unwrap_or(face.connected())
    }
    fn keep_alive(&mut self, face: Face) {
        crate::glue::contraptions::actors::pfi_keep_flow(self.0, face.connected());
    }
}

/// Up to `n` mB of `fluid` into end `to` of `pump`'s pipes in front: a hose pulley's or creative
/// tank's sink, a basin's inputs, a boiler's supply, a spout's tank, a tank's, or an open end's
/// space (a source placed per bucket, `provideFluidToSpace`). How much it took.
fn fill_end(lv: &mut ServerLevel<'_>, pos: [i32; 3], fluid: Fluid, n: u32) -> u32 {
    let dim = lv.dim;
    if let Some(took) = plumbing::sink(lv, pos, fluid, n) {
        return took;
    }
    if info_at(lv, pos).map(|(b, _)| b) == Some(Block::Basin) {
        // A basin takes into its inputs.
        let mut t = processing::basin_tanks(dim, pos);
        let took = create_engine::basin::fill(&mut t.input, fluid, n);
        if took > 0 {
            with_world(|w| w.set_basin_fluids(dim, pos, t));
        }
        return took;
    }
    with_world(|w| {
        // A boiler takes all the water as supply, none into its tank
        // (`BoilerFluidHandler.fill`).
        if let Some(took) = w.fill_boiler(dim, pos, fluid, n) {
            return took;
        }
        // A tank block fills its tank (by its controller).
        match w.dims[dim].spouts.get_mut(&pos) {
            Some(s) => {
                let took = create_engine::spout::fill(s, fluid, n);
                if took > 0 {
                    w.dims[dim].analog_checks.push(pos);
                }
                took
            }
            None => w.fill_tank(dim, pos, fluid, n),
        }
    })
    .unwrap_or(0)
}

/// Each column owns its pipe Connections compounds, including dry pressure.
pub(super) fn chunk_data_save(dim: usize, chunk: [i32; 2], data: &mut Compound) {
    let pipes = with_world(|w| {
        w.dims[dim]
            .fluid_transport
            .pipes
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]))
            .map(|(p, b)| (*p, b.clone()))
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    let records = pipes
        .into_iter()
        .map(|(p, b)| {
            let mut d = super::connections::encode(&b);
            d.insert("Pos".into(), Tag::IntArray(p.to_vec()));
            Tag::Compound(d)
        })
        .collect();
    data.insert("Connections".into(), Tag::List(records));
}
pub(super) fn chunk_load(dim: usize, data: &Compound) {
    if let Some(Tag::List(records)) = data.get(&"Connections".into()) {
        for record in records {
            let Tag::Compound(d) = record else {
                continue;
            };
            let Some(Tag::IntArray(p)) = d.get(&"Pos".into()) else {
                continue;
            };
            let [x, y, z] = p.as_slice() else {
                continue;
            };
            if let Some(b) = super::connections::decode(d) {
                with_world(|w| w.dims[dim].fluid_transport.pipes.insert([*x, *y, *z], b));
            }
        }
    }
    if data.contains_key(&"Connections".into()) {
        return;
    }
    if let Some(Tag::IntArray(records)) = data.get(&"open_ends".into()) {
        let keys = super::fluid_keys(data, "open_ends");
        for c in records.as_chunks::<7>().0 {
            let side = Dir::ALL[c[3].clamp(0, 5) as usize];
            let pos = [c[0], c[1], c[2]];
            let tank = create_engine::tank::Codes::fluid(&keys, c[4])
                .filter(|_| c[5] > 0)
                .map(|f| (f, c[5] as u32));
            with_world(|w| {
                let pipe = w.dims[dim]
                    .fluid_transport
                    .pipes
                    .entry(pos)
                    .or_insert_with(|| {
                        create_engine::fluids::transport::FluidTransportBehaviour::new(side.bit())
                    });
                if pipe.connection(side).is_none() {
                    pipe.interfaces.push((side, Default::default()));
                }
                if let Some(connection) = pipe.connection(side) {
                    connection.set_source(Source::open(create_engine::open_end::OpenEnd {
                        tank,
                        pulling: c[6] != 0,
                    }));
                }
                w.dims[dim]
                    .fluid_transport
                    .changed
                    .push(create_engine::fluids::runtime::Change::Pipe(pos));
            });
        }
    }
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
