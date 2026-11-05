// Create's fluids in the game: pipes and pumps, tanks, spouts and item drains. A sibling of the
// `create` module (`runtime.rs`).
use super::Game;
use super::create::*;
use super::create_logistics::*;
use super::create_processing::*;
use create_engine::kinetics::Dir;
use horizon_world::registry::BlockStateId;

/// Spouts holding fluid (`SpoutBlockEntity.whenItemHeld` on the depot two below, `tick`): the
/// held empty bucket is filled after the 20-tick cycle (at its 5th-last tick).
pub(super) fn tick_spouts(game: &mut Game<'_>, dim: usize) {
    let spouts: Vec<([i32; 3], create_engine::spout::Spout)> = with_world(|w| {
        w.dims[dim]
            .spouts
            .iter()
            .filter(|(_, s)| s.fluid.is_some() || s.processing >= 0)
            .map(|(p, s)| (*p, *s))
            .collect()
    })
    .unwrap_or_default();
    for (pos, mut s) in spouts {
        let before = s.fluid;
        let depot = [pos[0], pos[1] - 2, pos[2]];
        let on_depot = with_world(|w| w.dims[dim].depots.contains_key(&depot)).unwrap_or(false);
        let held = on_depot
            .then(|| depot_items(game, depot).into_iter().find(|x| x.0 == 0))
            .flatten();
        if !ensure_recipes(game) {
            continue;
        }
        // A sequenced assembly's filling step first (`SequencedAssemblyRecipe.getRecipe`): its
        // fluid and amount, if the spout holds that fluid.
        let sequenced = held
            .as_ref()
            .and_then(|_| sequenced_on_depot(game, depot, "create:filling", None))
            .and_then(|(a, progress, step)| {
                let (fluid, amount) = step.fluid?;
                let fluid = create_engine::tank::Fluid::of_id(&fluid)?;
                (s.fluid.map(|f| f.0) == Some(fluid)).then_some((a, progress, fluid, amount))
            });
        let filled = RECIPES.with_borrow(|r| {
            let r = r.as_ref()?;
            let item = held.as_ref().map(|h| h.1.as_str());
            match &sequenced {
                Some((a, _, fluid, amount)) => {
                    let step = [create_engine::spout::Filling {
                        item: item?.to_string(),
                        fluid: *fluid,
                        amount: *amount,
                        result: a.transitional.clone(),
                    }];
                    create_engine::spout::step(&mut s, item, &step)
                }
                None => create_engine::spout::step(&mut s, item, &r.filling),
            }
        });
        if let (Some(_), Some((a, progress, _, _)), Some((_, _, count))) =
            (&filled, &sequenced, &held)
        {
            let state = with_world(|w| w.dims[dim].depots.get(&depot).copied().flatten()).flatten();
            if let Some(state) = state {
                let next =
                    sequenced_step_on_depot(game, dim, depot, *count, state, a, progress.as_ref());
                with_world(|w| {
                    if let Some(h) = w.dims[dim].depots.get_mut(&depot) {
                        *h = Some(next);
                    }
                });
            }
        } else if let (Some(out), Some((_, item, count))) = (filled, held) {
            let out = out.as_str();
            // `convertToAndLeaveHeld`: one is filled; the rest stays held, the filled one goes to
            // the outputs.
            game.native_with_mod_block_entity(depot, |be| {
                be.items
                    .resize(be.items.len().max(create_engine::depot::SLOTS), None);
                let stack = |i: &str, n: u32| horizon_client::items::ItemStack {
                    item: i.into(),
                    count: n,
                    damage: 0,
                    extra: None,
                };
                if count <= 1 {
                    be.items[0] = Some(stack(out, 1));
                } else {
                    be.items[0] = Some(stack(&item, u32::from(count) - 1));
                    // Into the outputs as the depot stacks them (`insertItemStacked`).
                    let outputs = &mut be.items[create_engine::depot::OUTPUTS];
                    if let Some(s) = outputs
                        .iter_mut()
                        .flatten()
                        .find(|s| &*s.item == out && s.count < 64)
                    {
                        s.count += 1;
                    } else if let Some(slot) = outputs.iter_mut().find(|s| s.is_none()) {
                        *slot = Some(stack(out, 1));
                    }
                }
            });
            with_world(|w| w.depot_changed(dim, depot));
        }
        with_world(|w| w.dims[dim].spouts.insert(pos, s));
        if s.fluid != before {
            analog_touched(dim, pos);
        }
    }
}

/// Item drains holding items (`ItemDrainBlockEntity.tick`): a filled bucket is emptied into the
/// tank each cycle while it has room; the empty bucket is thrown off on top; anything else is
/// let go the same way.
pub(super) fn tick_drains(game: &mut Game<'_>, dim: usize) {
    let drains: Vec<[i32; 3]> = with_world(|w| {
        w.dims[dim]
            .drains
            .iter()
            .filter(|(_, d)| d.held.is_some())
            .map(|(p, _)| *p)
            .collect()
    })
    .unwrap_or_default();
    if !drains.is_empty() && !ensure_recipes(game) {
        return;
    }
    for pos in drains {
        let Some(out) = RECIPES.with_borrow(|r| {
            let emptying = r.as_ref().map_or(&[][..], |r| &r.emptying[..]);
            with_world(|w| {
                let d = w.dims[dim].drains.get_mut(&pos)?;
                let held = d.held.clone();
                if held
                    .as_ref()
                    .is_some_and(|h| create_engine::drain::contents(&h.0, emptying).is_none())
                {
                    d.held = None;
                    return held.map(|h| (h.0, h.1));
                }
                let before = d.fluid;
                let out = create_engine::drain::tick(d, emptying);
                if d.fluid != before {
                    w.dims[dim].analog_checks.push(pos);
                }
                out?.map(|left| (left, 1))
            })
            .flatten()
        }) else {
            continue;
        };
        let (x, y, z) = (
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]) + 1.0,
            f64::from(pos[2]) + 0.5,
        );
        game.native_run_command(&format!(
            "summon minecraft:item {x} {y} {z} {{Item:{{id:'{}',count:{}}},PickupDelay:20}}",
            out.0, out.1
        ));
    }
}

/// What a block is to the pipes (`FluidPropagator`): a pipe and its open sides, a pump and its
/// facing, a tank, open space a pipe may end in, or neither.
pub(super) fn pipe_cell(game: &Game<'_>, p: [i32; 3]) -> create_engine::fluids::Cell {
    use create_engine::Block;
    use create_engine::fluids::Cell;
    if !game.world.loaded(p[0], p[2]) {
        return Cell::Solid;
    }
    let state = game.world.block(p[0], p[1], p[2]);
    if let Some((block, info)) =
        with_world(|w| w.table.get(state.0).map(|(b, i)| (b, *i))).flatten()
    {
        return match block {
            Block::FluidPipe => Cell::Pipe(pipe_sides_of(game, state)),
            Block::MechanicalPump => info.facing.map_or(Cell::Solid, Cell::Pump),
            Block::FluidTank | Block::Spout | Block::ItemDrain | Block::Basin => Cell::Tank,
            b => super::create_plumbing::plumbing_cell(game, state, b, &info, p)
                .unwrap_or(Cell::Solid),
        };
    }
    let b = game.res.behaviours();
    if b.is_replaceable(state) || b.fluid(state).kind() != horizon_world::fluid::FluidKind::Empty {
        Cell::Open
    } else {
        Cell::Solid
    }
}

/// A pipe state's open sides (`Dir::bit`).
pub(super) fn pipe_sides_of(game: &Game<'_>, state: BlockStateId) -> u8 {
    let Some(s) = game.pack.assets.registry.state(state) else {
        return 0;
    };
    Dir::ALL
        .into_iter()
        .filter(|d| s.properties.get(dir_name(*d)).is_some_and(|v| v == "true"))
        .fold(0, |m, d| m | d.bit())
}

/// `FluidPipeBlock.updateBlockState` for the pipe at `pos`, written when it changes.
pub(super) fn connect_pipe(
    game: &mut Game<'_>,
    dimension: &str,
    pos: [i32; 3],
    preferred: Option<Dir>,
) {
    use create_engine::fluids;
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let is_pipe = with_world(|w| w.table.get(state.0).map(|(b, _)| b)).flatten()
        == Some(create_engine::Block::FluidPipe);
    if !is_pipe {
        return;
    }
    let previous = pipe_sides_of(game, state);
    let sides = fluids::pipe_sides(
        previous,
        |d| {
            let o = d.offset();
            pipe_cell(game, [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]])
        },
        preferred.unwrap_or(Dir::North),
    );
    if sides == previous && preferred.is_none() {
        return;
    }
    let Some(mut s) = game.pack.assets.registry.state(state).cloned() else {
        return;
    };
    for d in Dir::ALL {
        let open = if sides & d.bit() != 0 {
            "true"
        } else {
            "false"
        };
        s.properties.insert(dir_name(d).to_owned(), open.to_owned());
    }
    if let Some(id) = game.pack.assets.registry.id(&s)
        && id != state
    {
        game.native_set_block(dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
    }
}

/// Pipes and pumps: pipes that changed connect again; turning pumps find their ends when pipes
/// changed (`distributePressureTo`) and carry fluid from the first end behind them to the
/// first end in front that takes it, `max(1, pressure / 2)` mB a tick (`FluidNetwork`).
pub(super) fn tick_pipes(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::fluids;
    use create_engine::tank::BUCKET;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].pipe_checks)).unwrap_or_default();
    checks.sort();
    checks.dedup();
    for pos in checks {
        connect_pipe(game, dimension, pos, None);
    }
    let changed =
        with_world(|w| std::mem::replace(&mut w.dims[dim].pipes_changed, false)).unwrap_or(false);
    let pumps = with_world(|w| w.dims[dim].pumping.clone()).unwrap_or_default();
    for pos in pumps {
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let Some(facing) =
            with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten()
        else {
            continue;
        };
        let Some(mut pump) = with_world(|w| w.dims[dim].pumps.get(&pos).cloned()).flatten() else {
            continue;
        };
        if changed || !pump.found {
            pump.pull = fluids::reach(pos, facing.opposite(), speed, |p| pipe_cell(game, p));
            pump.push = fluids::reach(pos, facing, speed, |p| pipe_cell(game, p));
            pump.found = true;
            // `FluidNetwork.reset`: the flows start again from the pump.
            pump.pumped = 0;
        }
        // The first end behind it no closed valve shuts off.
        let from = pump.pull.iter().find(|e| !e.blocked).copied();
        if let (Some(from), false) = (from, pump.push.is_empty()) {
            let amount = fluids::transfer_speed(from.pressure);
            // What the source can give this tick: a creative tank or a hose pulley's, else...
            let special = if from.tank {
                super::create_plumbing::source(game, dimension, dim, from.pos, amount)
            } else {
                None
            };
            let drained_from_drain = from.tank
                && with_world(|w| w.dims[dim].drains.contains_key(&from.pos)).unwrap_or(false);
            let from_basin = from.tank && basin_at(game, from.pos);
            let offer = if let Some(offer) = special {
                offer
            } else if from_basin {
                // A basin gives from its outputs.
                let mut t = basin_tanks(dim, from.pos);
                create_engine::basin::drain(&mut t.output, amount)
            } else if drained_from_drain {
                with_world(|w| {
                    w.dims[dim]
                        .drains
                        .get(&from.pos)
                        .and_then(|d| d.fluid.map(|(f, n)| (f, n.min(amount))))
                })
                .flatten()
            } else if from.tank {
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
                if pump.buffer.is_none() {
                    // `OpenEndedPipe.removeFluidFromSpace`: a source block gives a bucket's worth.
                    let s = game.world.block(from.pos[0], from.pos[1], from.pos[2]);
                    let f = game.res.behaviours().fluid(s);
                    let kind = f.kind();
                    use horizon_world::fluid::FluidKind;
                    let fluid =
                        fluids::open_fluid(kind == FluidKind::Water, kind == FluidKind::Lava);
                    if let Some(fluid) = fluid.filter(|_| f.is_source()) {
                        pump.buffer = Some((fluid, BUCKET));
                        let name = if fluid == create_engine::tank::Fluid::Water {
                            "minecraft:water"
                        } else {
                            "minecraft:lava"
                        };
                        if let Some(left) =
                            state_with(game.pack.assets.registry, name, &[("level", "14")])
                        {
                            game.native_set_block(
                                dimension,
                                from.pos,
                                left,
                                super::native_hooks::flags::UPDATE_ALL,
                            );
                        }
                    }
                }
                pump.buffer.map(|(f, n)| (f, n.min(amount)))
            };
            // Smart pipes on the way let only their filter's fluid through.
            let offer = offer.filter(|(f, _)| from.passes(*f));
            if let Some((fluid, n)) = offer {
                // The flows on the way advance (`manageFlows`); the ends in front they have
                // reached take an equal share each (`FluidNetwork.tick`'s `availableOutputs`).
                pump.pumped = pump.pumped.saturating_add(1);
                // No list of them is made: the k-th reached end is found when it is filled.
                let push = std::mem::take(&mut pump.push);
                let pumped = pump.pumped;
                let reaches = |to: &&fluids::End| {
                    to.passes(fluid) && pumped >= fluids::arrival_ticks(&from, to, speed)
                };
                let count = push.iter().filter(reaches).count();
                let moved = fluids::share(count, n, |k, give| {
                    let Some(to) = push.iter().filter(reaches).nth(k) else {
                        return 0;
                    };
                    fill_end(game, dimension, dim, to, fluid, give, &mut pump)
                });
                pump.push = push;
                if moved > 0 {
                    if special.is_some() {
                        super::create_plumbing::take(game, dimension, dim, from.pos, moved);
                    } else if from_basin {
                        let mut t = basin_tanks(dim, from.pos);
                        create_engine::basin::drain(&mut t.output, moved);
                        with_world(|w| w.set_basin_fluids(dim, from.pos, t));
                    } else if drained_from_drain {
                        with_world(|w| {
                            if let Some(d) = w.dims[dim].drains.get_mut(&from.pos) {
                                create_engine::drain::drain(d, moved);
                                w.dims[dim].analog_checks.push(from.pos);
                            }
                        });
                    } else if from.tank {
                        with_world(|w| w.drain_tank(dim, from.pos, moved));
                    } else if let Some((f, left)) = pump.buffer {
                        pump.buffer = (left > moved).then_some((f, left - moved));
                    }
                    with_world(|w| w.mark_unsaved(dim, pos));
                }
            }
        }
        with_world(|w| {
            w.dims[dim].pumps.insert(pos, pump);
        });
    }
}

/// Up to `n` mB of `fluid` into end `to` of `pump`'s pipes in front: a hose pulley's or creative
/// tank's sink, a basin's inputs, a boiler's supply, a spout's tank, a tank's, or an open end's
/// space (a source placed per bucket, `provideFluidToSpace`). How much it took.
fn fill_end(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    to: &create_engine::fluids::End,
    fluid: create_engine::tank::Fluid,
    n: u32,
    pump: &mut create_engine::world::Pump,
) -> u32 {
    use create_engine::tank::BUCKET;
    let special = if to.tank {
        super::create_plumbing::sink(game, dimension, dim, to.pos, fluid, n)
    } else {
        None
    };
    if let Some(took) = special {
        return took;
    }
    if to.tank && basin_at(game, to.pos) {
        // A basin takes into its inputs.
        let mut t = basin_tanks(dim, to.pos);
        let took = create_engine::basin::fill(&mut t.input, fluid, n);
        if took > 0 {
            with_world(|w| w.set_basin_fluids(dim, to.pos, t));
        }
        return took;
    }
    if to.tank {
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
    // `OpenEndedPipe.provideFluidToSpace`: a bucket's worth becomes a source.
    let have = pump.out.filter(|o| o.0 == fluid).map_or(0, |o| o.1);
    let total = have + n;
    if !fluid.placeable() {
        // Create's own fluids have no blocks in the world yet.
        return 0;
    }
    if total < BUCKET {
        pump.out = Some((fluid, total));
        return n;
    }
    let name = if fluid == create_engine::tank::Fluid::Water {
        "minecraft:water"
    } else {
        "minecraft:lava"
    };
    let here = game.world.block(to.pos[0], to.pos[1], to.pos[2]);
    if !game.res.behaviours().is_replaceable(here) {
        return 0;
    }
    if let Some(src) = state_with(game.pack.assets.registry, name, &[("level", "0")]) {
        game.native_set_block(
            dimension,
            to.pos,
            src,
            super::native_hooks::flags::UPDATE_ALL,
        );
    }
    pump.out = (total > BUCKET).then_some((fluid, total - BUCKET));
    n
}

/// Boilers and steam engines (`BoilerData.tick`, `SteamEngineBlockEntity.tick`): tanks whose
/// engines changed count them again, shafts in front of engines become powered shafts or plain
/// ones again, boilers whose heaters changed read them, and boilers sample their water supply.
/// Nothing runs without a boiler or a changed engine.
pub(super) fn tick_boilers(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let changes = with_world(|w| {
        w.evaluate_boilers(dim);
        w.shaft_changes(dim)
    })
    .unwrap_or_default();
    for (pos, powered) in changes {
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let Some(s) = game.pack.assets.registry.state(state) else {
            continue;
        };
        // `PoweredShaftBlock.getEquivalent` and its way back: axis and waterlogging kept.
        let axis = s.properties.get("axis").cloned().unwrap_or_default();
        let water = s.properties.get("waterlogged").cloned().unwrap_or_default();
        let name = if powered {
            "create:powered_shaft"
        } else {
            "create:shaft"
        };
        if let Some(id) = state_with(
            game.pack.assets.registry,
            name,
            &[("axis", &axis), ("waterlogged", &water)],
        ) {
            game.native_set_block(dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
        }
    }
    for (tank, width) in with_world(|w| w.take_heat_checks(dim)).unwrap_or_default() {
        // `updateTemperature`: every block under the tank's bottom layer.
        let w = i32::from(width);
        let heats: Vec<i32> = (0..w)
            .flat_map(|x| (0..w).map(move |z| (x, z)))
            .map(|(x, z)| boiler_heater(game, [tank[0] + x, tank[1] - 1, tank[2] + z]))
            .collect();
        with_world(|w| w.set_boiler_heat(dim, tank, &heats));
    }
    with_world(|w| w.tick_boilers(dim));
}

/// `BoilerHeater.findHeat` under a boiler: a blaze burner by its heat (`BoilerHeaters
/// .blazeBurner`), another block in `create:passive_boiler_heaters` passive unless it is out
/// (`BoilerHeaters.passive`: an unlit campfire), anything else none.
pub(super) fn boiler_heater(game: &Game<'_>, p: [i32; 3]) -> i32 {
    use create_engine::steam;
    let state = game.world.block(p[0], p[1], p[2]);
    if let Some((block, heat)) =
        with_world(|w| w.table.get(state.0).map(|(b, i)| (b, i.heat))).flatten()
        && block == create_engine::Block::BlazeBurner
    {
        return heat.map_or(steam::NO_HEAT, steam::blaze_burner_heat);
    }
    let b = game.res.behaviours();
    if !b
        .tag("create:passive_boiler_heaters")
        .is_some_and(|t| t.contains(b.block(state)))
    {
        return steam::NO_HEAT;
    }
    let lit = game
        .pack
        .assets
        .registry
        .state(state)
        .and_then(|s| s.properties.get("lit").cloned());
    steam::passive_heat(lit.as_deref() != Some("false"))
}

/// `SteamEngineBlock.getStateForPlacement`: a placed engine goes on the tank nearest the
/// player's view (floor, wall or ceiling), turned to it. With no tank around it stays as placed
/// (Create refuses the placement).
pub(super) fn place_engine(game: &mut Game<'_>, pos: [i32; 3], state: BlockStateId) {
    use create_engine::place::{horizontal_direction, ordered_by_nearest};
    let tanks: Vec<Dir> = Dir::ALL
        .into_iter()
        .filter(|d| {
            let o = d.offset();
            let s = game
                .world
                .block(pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]);
            with_world(|w| w.table.get(s.0).map(|(b, _)| b)).flatten()
                == Some(create_engine::Block::FluidTank)
        })
        .collect();
    let Some((face, facing)) = create_engine::steam::placement(
        ordered_by_nearest(game.yaw, game.pitch),
        horizontal_direction(game.yaw),
        |d| tanks.contains(&d),
    ) else {
        return;
    };
    let Some(mut s) = game.pack.assets.registry.state(state).cloned() else {
        return;
    };
    s.properties.insert("face".into(), face.name().into());
    s.properties
        .insert("facing".into(), dir_name(facing).into());
    if let Some(id) = game.pack.assets.registry.id(&s)
        && id != state
    {
        let dimension = game.dim.id().to_owned();
        game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
    }
}

/// Tanks saved with a column: `tank_multis` holds x, y, z, fluid, amount, width, height,
/// windows for each controller; `tanks` (before multiblock tanks) x, y, z, fluid, amount for
/// one-block tanks.
pub(super) fn tanks_from_nbt(
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<([i32; 3], create_engine::tank::Multi)> {
    use create_engine::tank::{Fluid, MAX_HEIGHT, MAX_WIDTH, Multi, Tank};
    use horizon_world::nbt::Tag;
    let tank = |c: &[i32]| Tank {
        fluid: Fluid::from_index(c[3].clamp(0, 255) as u8),
        amount: c[4].max(0) as u32,
    };
    let mut out = Vec::new();
    if let Some(Tag::IntArray(v)) = data.and_then(|d| d.get(&"tank_multis".into())) {
        out.extend(v.chunks_exact(8).map(|c| {
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
    if let Some(Tag::IntArray(v)) = data.and_then(|d| d.get(&"tanks".into())) {
        out.extend(v.chunks_exact(5).map(|c| {
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

/// `FluidTankBlock.useItemOn` for a creative player (Create 6 leaves survival to spouts and
/// drains): a filled bucket empties into the tank, an empty one takes a bucket's worth (and
/// is given, filled, unless the player has one already), each only whole.
pub(super) fn use_tank(game: &mut Game<'_>, dim: usize, pos: [i32; 3], canceled: &mut bool) {
    use create_engine::tank::Fluid;
    if !game.creative {
        return;
    }
    let Some((held, _, _)) = game.native_held() else {
        return;
    };
    let Some((_, multi)) = with_world(|w| w.tank(dim, pos)).flatten() else {
        return;
    };
    let mut t = multi.tank;
    // A boiler's handler is the boiler's (`FluidTankBlockEntity.getFluidCapability`,
    // `BoilerFluidHandler`): a water bucket becomes supply, other buckets are refused and an
    // empty one gets nothing, all still used up (`useItemOn` returns SUCCESS).
    let bucket = Fluid::of_bucket(&held);
    if bucket.is_some() || held == "minecraft:bucket" {
        let boiler = with_world(|w| match bucket {
            Some(f) => w
                .fill_boiler(dim, pos, f, create_engine::tank::BUCKET)
                .is_some(),
            None => w.is_boiler(dim, pos),
        })
        .unwrap_or(false);
        if boiler {
            *canceled = true;
            return;
        }
    }
    let changed = if let Some(f) = bucket {
        t.fill(f, multi.capacity())
    } else if held == "minecraft:bucket" {
        match t.drain() {
            Some(f) => {
                if game.native_count_items(f.bucket()) == 0 {
                    game.native_give_items(f.bucket(), 1);
                }
                true
            }
            None => false,
        }
    } else {
        return;
    };
    *canceled = true;
    if changed {
        with_world(|w| w.set_tank(dim, pos, t));
    }
}

pub(super) fn send_tank_changes(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let changed = with_world(|w| {
        let mut v = std::mem::take(&mut w.dims[dim].tank_changes);
        v.sort();
        v.dedup();
        v
    })
    .unwrap_or_default();
    if changed.is_empty() {
        return;
    }
    // Their comparators read the tank's fill again (`onFluidStackChanged`: `setChanged`).
    with_world(|w| w.dims[dim].analog_checks.extend_from_slice(&changed));
    let players = players_in(game, dimension);
    send_tanks(game, &players, dim, &changed);
}

pub(super) fn send_tanks(game: &mut Game<'_>, players: &[u128], dim: usize, tanks: &[[i32; 3]]) {
    // A controller's tank; none for a block that is no controller (anymore).
    let all: Vec<([i32; 3], Option<create_engine::tank::Multi>)> = with_world(|w| {
        tanks
            .iter()
            .map(|&p| (p, w.dims[dim].tanks.multis.get(&p).copied()))
            .collect()
    })
    .unwrap_or_default();
    for m in create_engine::tank::encode(&all) {
        for &p in players {
            send_to_player(game, p, create_engine::tank::CHANNEL, &m);
        }
    }
}

/// A tank's fluid on the client: `FluidTankRenderer`'s box over the whole tank from its
/// controller, the fluid's still texture tinted (water by its default colour), one part a tank,
/// uploaded when its contents or size change.
pub(super) fn tanks_from_server(game: &mut Game<'_>, data: &[u8]) {
    use create_engine::tank::Fluid;
    let registry = game.pack.assets.registry;
    for (pos, m) in create_engine::tank::decode(data) {
        let key = depot_key(pos, 210, 0);
        let Some((min, max)) = m.as_ref().and_then(create_engine::tank::fluid_box) else {
            game.native_part_clear(key);
            continue;
        };
        let (name, tint) = match m.and_then(|m| m.tank.fluid) {
            Some(Fluid::Water) => ("create:part_fluid_water", Some(0x3F_76E4)),
            Some(Fluid::Honey) => ("create:part_fluid_honey", None),
            Some(Fluid::Chocolate) => ("create:part_fluid_chocolate", None),
            Some(Fluid::Milk) => ("create:part_fluid_milk", None),
            _ => ("create:part_fluid_lava", None),
        };
        let Some(state) = registry.default_state(name).and_then(|d| {
            let mut s = registry.state(d)?.clone();
            s.properties.insert("facing".into(), "up".into());
            registry.id(&s)
        }) else {
            continue;
        };
        let pose = glam::Mat4::from_translation(glam::Vec3::from_array(min))
            * glam::Mat4::from_scale(glam::Vec3::from_array(max) - glam::Vec3::from_array(min));
        if let Some(model) = game.native_part_model_tinted(state, pose, tint) {
            game.native_part_set(key, model, pos, 1, 0.0, 0.0);
        }
    }
}

/// Tank blocks whose tank formed, split or became a boiler take its `top`, `bottom` and window
/// `shape` (`notifyMultiUpdated`, `setWindows`, `removeController`), with clients told and no
/// neighbour updates as Create sets them.
pub(super) fn tick_tank_states(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let changed = with_world(|w| w.take_tank_states(dim)).unwrap_or_default();
    for pos in changed {
        if !game.world.loaded(pos[0], pos[2]) {
            continue;
        }
        let Some(want) = with_world(|w| w.tank_shape(dim, pos)).flatten() else {
            continue;
        };
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let is_tank = with_world(|w| w.table.get(state.0).map(|(b, _)| b)).flatten()
            == Some(create_engine::Block::FluidTank);
        if !is_tank {
            continue;
        }
        let Some(mut s) = game.pack.assets.registry.state(state).cloned() else {
            continue;
        };
        let flag = |b: bool| if b { "true" } else { "false" };
        s.properties.insert("top".into(), flag(want.top).into());
        s.properties
            .insert("bottom".into(), flag(want.bottom).into());
        s.properties
            .insert("shape".into(), want.shape.name().into());
        if let Some(id) = game.pack.assets.registry.id(&s)
            && id != state
        {
            game.native_set_block(
                dimension,
                pos,
                id,
                super::native_hooks::flags::UPDATE_CLIENTS,
            );
        }
    }
}
