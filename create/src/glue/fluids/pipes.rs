//! Pipes and pumps (`FluidPropagator`, `FluidNetwork`), boilers and steam engines, and tank
//! blocks' states.

use super::super::processing;
use super::super::server::{ServerLevel, UPDATE_CLIENTS};
use super::super::world::with_world;
use super::{all, flag, has_fluid, info_at, plumbing, replaceable, state_with, world_fluid};
use crate::glue::kinetics::dir_name;
use create_engine::Block;
use create_engine::fluids::{self, Cell};
use create_engine::kinetics::Dir;
use create_engine::tank::{BUCKET, Fluid};
use mod_api::{BlockState, HostBlockStates};

/// What a block is to the pipes (`FluidPropagator`): a pipe and its open sides, a pump and its
/// facing, a tank, open space a pipe may end in, or neither.
pub(super) fn pipe_cell(lv: &ServerLevel<'_>, p: [i32; 3]) -> Cell {
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
    if replaceable(lv, p) || has_fluid(lv, p) {
        Cell::Open
    } else {
        Cell::Solid
    }
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
pub(super) fn tick_pipes(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].pipe_checks)).unwrap_or_default();
    checks.sort();
    checks.dedup();
    for pos in checks {
        connect_pipe(lv, pos, None);
    }
    let changed =
        with_world(|w| std::mem::replace(&mut w.dims[dim].pipes_changed, false)).unwrap_or(false);
    let pumps = with_world(|w| w.dims[dim].pumping.clone()).unwrap_or_default();
    for pos in pumps {
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
        pump.pull = fluids::reach(pos, facing.opposite(), speed, |p| pipe_cell(lv, p));
        pump.push = fluids::reach(pos, facing, speed, |p| pipe_cell(lv, p));
        let glass = |p| info_at(lv, p).is_some_and(|(b, _)| b == Block::GlassFluidPipe);
        pump.shown_pull =
            fluids::shown_on_the_way(pos, facing.opposite(), |p| pipe_cell(lv, p), glass);
        pump.shown_push = fluids::shown_on_the_way(pos, facing, |p| pipe_cell(lv, p), glass);
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
            plumbing::source(lv, from.pos, amount)
        } else {
            None
        };
        let drained_from_drain = from.tank
            && with_world(|w| w.dims[dim].drains.contains_key(&from.pos)).unwrap_or(false);
        let from_basin = from.tank && info_at(lv, from.pos).map(|(b, _)| b) == Some(Block::Basin);
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
                let fluid = world_fluid(lv, from.pos).filter(|(_, source)| *source);
                if let Some((fluid, _)) = fluid {
                    pump.buffer = Some((fluid, BUCKET));
                    if let Some(left) = state_with(lv, fluid.id(), &[("level", "14")]) {
                        lv.set_state(from.pos, left, all());
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
            let mut filled = Vec::new();
            let moved = fluids::share(count, n, |k, give| {
                let Some(to) = push.iter().filter(reaches).nth(k) else {
                    return 0;
                };
                let took = fill_end(lv, to, fluid, give, &mut pump);
                if took > 0 {
                    filled.push(to.pos);
                }
                took
            });
            if moved > 0 {
                // The glass pipes on the way show the fluid going through.
                let ways = pump
                    .shown_pull
                    .iter()
                    .filter(|(end, _)| *end == from.pos)
                    .chain(
                        pump.shown_push
                            .iter()
                            .filter(|(end, _)| filled.contains(end)),
                    );
                let now = lv.game_time();
                for (_, way) in ways {
                    super::glass::flowing(dim, way, fluid, now);
                }
                // Open ends pour the fluid out, or draw it in (`PipeConnection.spawnParticles`).
                if !from.tank {
                    pour(lv, from.pos, fluid, true);
                }
                for end in push.iter().filter(|e| !e.tank && filled.contains(&e.pos)) {
                    pour(lv, end.pos, fluid, false);
                }
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
                } else if from.tank {
                    with_world(|w| w.drain_tank(dim, from.pos, moved));
                } else if let Some((f, left)) = pump.buffer {
                    pump.buffer = (left > moved).then_some((f, left - moved));
                }
                with_world(|w| w.mark_unsaved(dim, pos));
            }
            pump.push = push;
        }
    }
    with_world(|w| {
        w.dims[dim].pumps.insert(pos, pump);
    });
}

/// Up to `n` mB of `fluid` into end `to` of `pump`'s pipes in front: a hose pulley's or creative
/// tank's sink, a basin's inputs, a boiler's supply, a spout's tank, a tank's, or an open end's
/// space (a source placed per bucket, `provideFluidToSpace`). How much it took.
fn fill_end(
    lv: &mut ServerLevel<'_>,
    to: &fluids::End,
    fluid: Fluid,
    n: u32,
    pump: &mut create_engine::world::Pump,
) -> u32 {
    let dim = lv.dim;
    if to.tank {
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
    // `OpenEndedPipe.provideFluidToSpace`: a bucket's worth becomes a source.
    let have = pump.out.filter(|o| o.0 == fluid).map_or(0, |o| o.1);
    let total = have + n;
    if !fluid.placeable() {
        // Milk, tea and potions have no block: the open end takes a millibucket at a time and
        // lets it act on what is in front (`OpenEndFluidHandler.fill`, `OpenPipeEffectHandler`).
        if !replaceable(lv, to.pos) {
            return 0;
        }
        open_effect(lv, to, fluid);
        return n.min(1);
    }
    if total < BUCKET {
        pump.out = Some((fluid, total));
        return n;
    }
    if !replaceable(lv, to.pos) {
        return 0;
    }
    if let Some(src) = state_with(lv, fluid.id(), &[("level", "0")]) {
        lv.set_state(to.pos, src, all());
    }
    pump.out = (total > BUCKET).then_some((fluid, total - BUCKET));
    n
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

/// `minecraft:block`, for the fluid poured at open pipe ends.
static BLOCK_PARTICLE: std::sync::OnceLock<mod_api::ParticleKey> = std::sync::OnceLock::new();

pub(super) fn register(r: &mut dyn mod_api::Registrar) {
    let _ = BLOCK_PARTICLE.set(r.particle_key("minecraft:block"));
}

thread_local! {
    static RNG: std::cell::RefCell<create_engine::processing::Rng> =
        std::cell::RefCell::new(create_engine::processing::Rng::new(0xF10D));
}

/// `FluidFX.spawnPouringLiquid` at the pipe ending in the open space `open`: a drop of `fluid`
/// (its block's particle, as Create's `FluidStackParticle` draws its still texture) leaving the
/// rim, or flying back into the pipe when it is drawn in (`inbound`). Fluids with no block (milk,
/// potions, tea) show none.
fn pour(lv: &mut ServerLevel<'_>, open: [i32; 3], fluid: Fluid, inbound: bool) {
    let Some(&key) = BLOCK_PARTICLE.get() else {
        return;
    };
    if !fluid.placeable() {
        return;
    }
    // The pipe whose side opens onto the space, and that side.
    let Some((pipe, side)) = Dir::ALL.into_iter().find_map(|d| {
        let o = d.offset();
        let q = [open[0] + o[0], open[1] + o[1], open[2] + o[2]];
        (pipe_sides_at(lv, q) & d.opposite().bit() != 0).then_some((q, d.opposite()))
    }) else {
        return;
    };
    let Some(state) = state_with(lv, fluid.id(), &[("level", "0")]) else {
        return;
    };
    let r = || RNG.with_borrow_mut(|r| f64::from(r.next_float()));
    let dir = side.offset().map(f64::from);
    // `offsetRandomly(ZERO, r, RIM_RADIUS * .75)` across the side, then out to the rim.
    let rim = (1.0 / 4.0 + 1.0 / 64.0) * 0.75;
    let out = 0.5 + r() / 4.0;
    let mut v = [0.0; 3];
    for i in 0..3 {
        let across = (r() - 0.5) * 2.0 * rim;
        v[i] = if dir[i] != 0.0 { 0.0 } else { across } + dir[i] * out;
    }
    let mut m = v.map(|c| c / 4.0);
    let center = pipe.map(|c| f64::from(c) + 0.5);
    let mut at = [0, 1, 2].map(|i| center[i] + v[i]);
    if inbound {
        at = [0, 1, 2].map(|i| at[i] + m[i]);
        m = [0, 1, 2].map(|i| (center[i] + dir[i] * 0.5 - at[i]) / 16.0);
    }
    at[1] -= 1.0 / 16.0;
    let options = mod_api::ParticleOptions::simple(key);
    let _ = lv
        .level
        .send_particles(&mut *lv.host, &options, Some(state), at, m, 1.0, 0);
}

/// Vanilla's undead (`#minecraft:undead`, which the mod API cannot read for entities): healing
/// hurts them and harming heals them.
const UNDEAD: [&str; 17] = [
    "minecraft:skeleton",
    "minecraft:stray",
    "minecraft:wither_skeleton",
    "minecraft:skeleton_horse",
    "minecraft:bogged",
    "minecraft:parched",
    "minecraft:zombie_horse",
    "minecraft:camel_husk",
    "minecraft:zombie",
    "minecraft:zombie_villager",
    "minecraft:zombified_piglin",
    "minecraft:zoglin",
    "minecraft:drowned",
    "minecraft:husk",
    "minecraft:zombie_nautilus",
    "minecraft:wither",
    "minecraft:phantom",
];

/// A potion's effects (`PotionContents.forEachEffect`): its potion's, then its custom ones as
/// saved (`MobEffectInstance`'s codec: id, amplifier, duration, ambient, show_particles,
/// show_icon).
fn potion_effects(
    c: &create_engine::potion::Contents,
) -> Vec<(String, i32, i32, bool, bool, bool)> {
    use mod_api::Tag;
    let mut out: Vec<_> = c
        .potion
        .as_deref()
        .and_then(create_engine::potion::registered)
        .map_or(&[][..], |i| create_engine::potion::POTIONS[i].2)
        .iter()
        .map(|(e, amp, d)| (format!("minecraft:{e}"), *amp, *d, false, true, true))
        .collect();
    let Ok((Tag::List(custom), _)) = Tag::from_network_bytes(&c.custom_effects, None) else {
        return out;
    };
    let int = |t: Option<&Tag>, or: i32| match t {
        Some(Tag::Byte(v)) => i32::from(*v),
        Some(Tag::Short(v)) => i32::from(*v),
        Some(Tag::Int(v)) => *v,
        _ => or,
    };
    for e in custom {
        let Tag::Compound(e) = e else { continue };
        let Some(Tag::String(id)) = e.get(&"id".into()) else {
            continue;
        };
        let visible = int(e.get(&"show_particles".into()), 1) != 0;
        out.push((
            id.to_string_lossy(),
            int(e.get(&"amplifier".into()), 0),
            int(e.get(&"duration".into()), 0),
            int(e.get(&"ambient".into()), 0) != 0,
            visible,
            int(e.get(&"show_icon".into()), i32::from(visible)) != 0,
        ));
    }
    out
}

/// What an open end letting out a fluid without a block does to the living in front
/// (`OpenEndedPipe`'s `aoe`: its space and the one below, two below from a pipe facing down):
/// a potion gives each its effects, instant ones at half strength (`PotionEffectHandler`);
/// builder's tea gives haste every 5 ticks (`TeaEffectHandler`); milk cures every 5 ticks
/// (`MilkEffectHandler`).
fn open_effect(lv: &mut ServerLevel<'_>, to: &fluids::End, fluid: Fluid) {
    let p = to.pos.map(f64::from);
    let below = if to.face == Dir::Down { 2.0 } else { 1.0 };
    let (lo, hi) = (
        [p[0], p[1] - below, p[2]],
        [p[0] + 1.0, p[1] + 1.0, p[2] + 1.0],
    );
    let every_fifth = lv.game_time() % 5 == 0;
    let effects = match fluid {
        Fluid::Potion(potion) => potion_effects(potion.contents),
        Fluid::Tea if every_fifth => vec![("minecraft:haste".into(), 0, 21, false, false, false)],
        Fluid::Milk if every_fifth => Vec::new(),
        _ => return,
    };
    let found = lv.level.entities_in(&*lv.host, lo, hi).unwrap_or_default();
    for e in found {
        let Some(living) = lv.host.living(e) else {
            continue;
        };
        if fluid == Fluid::Milk {
            living.remove_all_effects(&mut *lv.host);
            continue;
        }
        for (id, amp, duration, ambient, visible, icon) in &effects {
            let short = id.strip_prefix("minecraft:").unwrap_or(id);
            if matches!(short, "instant_health" | "instant_damage") {
                // `HealOrHarmMobEffect.applyInstantenousEffect` at 0.5.
                let undead = e
                    .kind(&*lv.host)
                    .is_some_and(|k| UNDEAD.contains(&k.as_str()));
                if (short == "instant_health") != undead {
                    living.heal(&mut *lv.host, (4 << amp.clamp(&0, &30)) as f32 * 0.5);
                } else {
                    let _ = e.hurt(
                        &mut *lv.host,
                        "minecraft:magic",
                        (6 << amp.clamp(&0, &30)) as f32 * 0.5,
                        None,
                        None,
                    );
                }
                continue;
            }
            if let Some(key) = lv.host.effect_key(id) {
                let effect =
                    mod_api::ActiveEffect::new(key, *duration, *amp, *ambient, *visible, *icon);
                living.add_effect(&mut *lv.host, &effect);
            }
        }
    }
}
