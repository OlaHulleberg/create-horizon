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
        let depot = [pos[0], pos[1] - 2, pos[2]];
        let on_depot = with_world(|w| w.dims[dim].depots.contains_key(&depot)).unwrap_or(false);
        let held = on_depot
            .then(|| depot_items(game, depot).into_iter().find(|x| x.0 == 0))
            .flatten();
        if !ensure_recipes(game) {
            continue;
        }
        let filled = RECIPES.with_borrow(|r| {
            let r = r.as_ref()?;
            create_engine::spout::step(&mut s, held.as_ref().map(|h| h.1.as_str()), &r.filling)
        });
        if let (Some(out), Some((_, item, count))) = (filled, held) {
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
                create_engine::drain::tick(d, emptying)?.map(|left| (left, 1))
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
            _ => Cell::Solid,
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
        }
        if let (Some(from), false) = (pump.pull.first().copied(), pump.push.is_empty()) {
            let amount = fluids::transfer_speed(from.pressure);
            // What the source can give this tick.
            let drained_from_drain = from.tank
                && with_world(|w| w.dims[dim].drains.contains_key(&from.pos)).unwrap_or(false);
            let from_basin = from.tank && basin_at(game, from.pos);
            let offer = if from_basin {
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
                    w.dims[dim]
                        .tanks
                        .get(&from.pos)
                        .and_then(|t| t.fluid.map(|f| (f, t.amount.min(amount))))
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
            if let Some((fluid, n)) = offer {
                // The first end in front that takes some.
                let mut moved = 0;
                for to in &pump.push {
                    moved = if to.tank && basin_at(game, to.pos) {
                        // A basin takes into its inputs.
                        let mut t = basin_tanks(dim, to.pos);
                        let took = create_engine::basin::fill(&mut t.input, fluid, n);
                        if took > 0 {
                            with_world(|w| w.set_basin_fluids(dim, to.pos, t));
                        }
                        took
                    } else if to.tank {
                        with_world(|w| {
                            let d = &mut w.dims[dim];
                            let took = match d.spouts.get_mut(&to.pos) {
                                Some(s) => create_engine::spout::fill(s, fluid, n),
                                None => d
                                    .tanks
                                    .get_mut(&to.pos)
                                    .map_or(0, |t| t.fill_amount(fluid, n)),
                            };
                            if took > 0 {
                                d.tank_changes.push(to.pos);
                            }
                            took
                        })
                        .unwrap_or(0)
                    } else {
                        // `OpenEndedPipe.provideFluidToSpace`: a bucket's worth becomes a source.
                        let have = pump.out.filter(|o| o.0 == fluid).map_or(0, |o| o.1);
                        let total = have + n;
                        if !fluid.placeable() {
                            // Create's own fluids have no blocks in the world yet.
                            0
                        } else if total >= BUCKET {
                            let name = if fluid == create_engine::tank::Fluid::Water {
                                "minecraft:water"
                            } else {
                                "minecraft:lava"
                            };
                            let here = game.world.block(to.pos[0], to.pos[1], to.pos[2]);
                            if !game.res.behaviours().is_replaceable(here) {
                                0
                            } else {
                                if let Some(src) =
                                    state_with(game.pack.assets.registry, name, &[("level", "0")])
                                {
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
                        } else {
                            pump.out = Some((fluid, total));
                            n
                        }
                    };
                    if moved > 0 {
                        break;
                    }
                }
                if moved > 0 {
                    if from_basin {
                        let mut t = basin_tanks(dim, from.pos);
                        create_engine::basin::drain(&mut t.output, moved);
                        with_world(|w| w.set_basin_fluids(dim, from.pos, t));
                    } else if drained_from_drain {
                        with_world(|w| {
                            if let Some(d) = w.dims[dim].drains.get_mut(&from.pos) {
                                create_engine::drain::drain(d, moved);
                            }
                        });
                    } else if from.tank {
                        with_world(|w| {
                            let d = &mut w.dims[dim];
                            if let Some(t) = d.tanks.get_mut(&from.pos) {
                                t.drain_amount(moved);
                                d.tank_changes.push(from.pos);
                            }
                        });
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

pub(super) fn tanks_from_nbt(
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<([i32; 3], create_engine::tank::Tank)> {
    let Some(horizon_world::nbt::Tag::IntArray(v)) = data.and_then(|d| d.get(&"tanks".into()))
    else {
        return Vec::new();
    };
    v.chunks_exact(5)
        .map(|c| {
            (
                [c[0], c[1], c[2]],
                create_engine::tank::Tank {
                    fluid: create_engine::tank::Fluid::from_index(c[3].clamp(0, 255) as u8),
                    amount: c[4].max(0) as u32,
                },
            )
        })
        .collect()
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
    let Some(mut t) = with_world(|w| w.dims[dim].tanks.get(&pos).copied()).flatten() else {
        return;
    };
    let changed = if let Some(f) = Fluid::of_bucket(&held) {
        t.fill(f)
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
        with_world(|w| {
            w.dims[dim].tanks.insert(pos, t);
            w.dims[dim].tank_changes.push(pos);
            w.mark_unsaved(dim, pos);
        });
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
    let players = players_in(game, dimension);
    send_tanks(game, &players, dim, &changed);
}

pub(super) fn send_tanks(game: &mut Game<'_>, players: &[u128], dim: usize, tanks: &[[i32; 3]]) {
    let all: Vec<([i32; 3], create_engine::tank::Tank)> = tanks
        .iter()
        .map(|&p| {
            (
                p,
                with_world(|w| w.dims[dim].tanks.get(&p).copied())
                    .flatten()
                    .unwrap_or_default(),
            )
        })
        .collect();
    for m in create_engine::tank::encode(&all) {
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::tank::CHANNEL, &m);
        }
    }
}

/// A tank's fluid on the client: `FluidTankRenderer`'s box, the fluid's still texture tinted
/// (water by its default colour), one part a tank, uploaded when its contents change.
pub(super) fn tanks_from_server(game: &mut Game<'_>, data: &[u8]) {
    use create_engine::tank::Fluid;
    let registry = game.pack.assets.registry;
    for (pos, t) in create_engine::tank::decode(data) {
        let key = depot_key(pos, 210, 0);
        let Some((min, max)) = create_engine::tank::fluid_box(&t) else {
            game.native_part_clear(key);
            continue;
        };
        let (name, tint) = match t.fluid {
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
