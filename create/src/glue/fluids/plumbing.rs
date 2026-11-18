//! Fluid valves, smart, glass and encased pipes, hose pulleys, creative fluid tanks and steam
//! whistles (`create_engine::{fluids, hose, whistle, plumbing}` hold the rules).

use super::super::server::ServerLevel;
use super::super::world::with_world;
use super::pipes::{connect_pipe, pipe_sides_at, pipe_sides_of};
use super::{HOSE, all, flag, info_at, offset, property, state_with, world_fluid};
use crate::glue::kinetics::dir_name;
use create_engine::Block;
use create_engine::fluids::{Cell, Gate, SmartFilter};
use create_engine::kinetics::{Axis, Dir};
use create_engine::parts::StateInfo;
use create_engine::tank::{Fluid, MAX_HEIGHT, MAX_WIDTH, Multi, Tank};
use mod_api::{BlockShapeKind, Compound, HostBlockStates, SoundSource, Tag};

/// What a valve, a smart, glass or encased pipe, a hose pulley or a creative tank is to the
/// pipes; None for other blocks.
pub(super) fn plumbing_cell(
    lv: &ServerLevel<'_>,
    block: Block,
    info: &StateInfo,
    pos: [i32; 3],
) -> Option<Cell> {
    use create_engine::fluids::{smart_pipe_axis, valve_pipe_axis};
    let dim = lv.dim;
    Some(match block {
        Block::EncasedFluidPipe => Cell::Encased(pipe_sides_at(lv, pos)),
        Block::GlassFluidPipe => Cell::Axis(info.axis?, Gate::Open),
        // FluidValveBlockEntity.ValvePipeBehaviour: nothing passes while closed.
        Block::FluidValve => {
            let gate = if info.enabled {
                Gate::Open
            } else {
                Gate::Closed
            };
            Cell::Axis(valve_pipe_axis(info.facing?, info.along_first), gate)
        }
        // SmartPipeBehaviour: only its filter's fluid passes.
        Block::SmartFluidPipe => {
            let wall = info.face == Some(create_engine::steam::Face::Wall);
            let gate =
                with_world(|w| w.dims[dim].plumbing.filters.get(&pos).map(|f| f.gate())).flatten();
            Cell::Axis(
                smart_pipe_axis(wall, info.facing?),
                gate.unwrap_or(Gate::Open),
            )
        }
        Block::HosePulley => Cell::Port(create_engine::hose::pipe_side(info.facing?)),
        Block::CreativeFluidTank | Block::PortableFluidInterface => Cell::Tank,
        _ => return None,
    })
}

/// A pump's source end that is a creative tank or a hose pulley: what it gives this tick (a
/// hose simulates its pull). None for other ends.
pub(super) fn source(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    amount: u32,
    only: Option<Fluid>,
) -> Option<Option<(Fluid, u32)>> {
    let dim = lv.dim;
    // A portable fluid interface gives what its contraption's tanks hold.
    if let Some(offer) = crate::glue::contraptions::actors::pfi_offer(lv, pos, amount, only) {
        return Some(offer);
    }
    // CreativeSmartFluidTank.drain: always full of its fluid, never emptied.
    if let Some(fluid) = with_world(|w| w.dims[dim].plumbing.creative_fluid(pos)).flatten() {
        return Some(fluid.map(|f| (f, amount)));
    }
    let mut hose = with_world(|w| w.dims[dim].plumbing.hoses.remove(&pos)).flatten()?;
    let now = lv.game_time();
    let out = hose.drain(&mut HoseLevel(lv), pos, amount, true, now);
    put_hose(dim, pos, hose);
    Some(out)
}

/// What came out of a creative tank or hose pulley the pump drew from (the hose pulls for
/// real).
pub(super) fn take(lv: &mut ServerLevel<'_>, pos: [i32; 3], moved: u32, fluid: Fluid) -> u32 {
    let dim = lv.dim;
    if with_world(|w| w.dims[dim].plumbing.creative_fluid(pos))
        .flatten()
        .flatten()
        == Some(fluid)
    {
        return moved;
    }
    let portable = crate::glue::contraptions::actors::pfi_drain(lv, pos, moved, fluid);
    let Some(mut hose) = with_world(|w| w.dims[dim].plumbing.hoses.remove(&pos)).flatten() else {
        return portable;
    };
    let now = lv.game_time();
    let took = hose
        .drain(&mut HoseLevel(lv), pos, moved, false, now)
        .map_or(0, |(_, n)| n);
    put_hose(dim, pos, hose);
    with_world(|w| w.mark_unsaved(dim, pos));
    took
}

/// A pump's end that is a creative tank (it takes everything, `CreativeSmartFluidTank.fill`)
/// or a hose pulley (`HosePulleyFluidHandler.fill`): what it took. None for other ends.
pub(super) fn sink(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    fluid: Fluid,
    amount: u32,
) -> Option<u32> {
    let dim = lv.dim;
    if let Some(took) = crate::glue::contraptions::actors::pfi_fill(lv, pos, fluid, amount) {
        return Some(took);
    }
    if with_world(|w| w.dims[dim].plumbing.creative.contains_key(&pos)) == Some(true) {
        return Some(amount);
    }
    let mut hose = with_world(|w| w.dims[dim].plumbing.hoses.remove(&pos)).flatten()?;
    let now = lv.game_time();
    // `dimensionType().ultraWarm()`: water boils away there.
    let hot = lv
        .level
        .dimension_type(&*lv.host)
        .is_some_and(|t| t.water_evaporates);
    let took = hose.fill(&mut HoseLevel(lv), pos, fluid, amount, hot, now);
    put_hose(dim, pos, hose);
    if took > 0 {
        with_world(|w| w.mark_unsaved(dim, pos));
    }
    Some(took)
}

/// Capability fill simulation for FluidNetwork's preflight pass.
pub(super) fn simulate_sink(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    fluid: Fluid,
    amount: u32,
) -> Option<u32> {
    if let Some(took) =
        crate::glue::contraptions::actors::pfi_fill_action(lv, pos, fluid, amount, false)
    {
        return Some(took);
    }
    let dim = lv.dim;
    if with_world(|w| w.dims[dim].plumbing.creative.contains_key(&pos)) == Some(true) {
        return Some(amount);
    }
    let mut hose = with_world(|w| w.dims[dim].plumbing.hoses.remove(&pos)).flatten()?;
    let hot = lv
        .level
        .dimension_type(&*lv.host)
        .is_some_and(|t| t.water_evaporates);
    let now = lv.game_time();
    let took = hose.fill_action(&mut HoseLevel(lv), pos, fluid, amount, hot, now, true);
    put_hose(dim, pos, hose);
    Some(took)
}

/// A hose taken out to work on the world goes back, ticking if it has something to do.
fn put_hose(dim: usize, pos: [i32; 3], hose: create_engine::hose::Hose) {
    with_world(|w| {
        let p = &mut w.dims[dim].plumbing;
        p.hoses.insert(pos, hose);
        p.hose_busy(pos);
    });
}

/// The world as a hose pulley sees it, through the level in place.
struct HoseLevel<'l, 'a>(&'l mut ServerLevel<'a>);

impl create_engine::hose::Level for HoseLevel<'_, '_> {
    fn hose_worked(&mut self, hose: [i32; 3], lava: bool) {
        crate::glue::advancements::award_placer(self.0, hose, "hose_pulley");
        if lava {
            crate::glue::advancements::award_placer(self.0, hose, "hose_pulley_lava");
        }
    }

    fn loaded(&self, p: [i32; 3]) -> bool {
        self.0.is_loaded(p)
    }

    fn space(&self, p: [i32; 3]) -> create_engine::hose::Space {
        use create_engine::hose::Space;
        let lv = &*self.0;
        let Some(state) = lv.block_state(p) else {
            return Space::Solid;
        };
        let name = state.block().id(&*lv.host);
        if matches!(
            name.as_str(),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        ) {
            return Space::Replaceable;
        }
        let fluid = world_fluid(lv, p);
        if let Some(w) = property(lv, p, "waterlogged") {
            return Space::Waterloggable(w == "true");
        }
        // A fluid's own block (`LiquidBlock`: its `level`).
        if let Some((fluid, source)) = fluid
            && property(lv, p, "level").is_some()
        {
            return Space::Liquid(fluid, source);
        }
        let open = lv
            .level
            .block_shape(&*lv.host, state, p, BlockShapeKind::Collision)
            .is_some_and(|s| s.is_empty());
        if let Some((fluid, _)) = fluid {
            return if open {
                Space::Holding(fluid)
            } else {
                Space::Solid
            };
        }
        // `canBeReplacedByFluid`: what doesn't block motion, but signs, ladders, sugar cane,
        // bubble columns, portals and structure voids.
        let kept = matches!(
            name.as_str(),
            "minecraft:sugar_cane"
                | "minecraft:nether_portal"
                | "minecraft:end_portal"
                | "minecraft:end_gateway"
                | "minecraft:structure_void"
        );
        if open && !kept {
            Space::Replaceable
        } else {
            Space::Solid
        }
    }

    fn passable(&self, p: [i32; 3]) -> bool {
        super::replaceable(self.0, p)
    }

    fn empty(&mut self, p: [i32; 3]) {
        let lv = &mut *self.0;
        let to = if property(lv, p, "waterlogged").is_some() {
            let meta = lv.host.block_states();
            lv.block_state(p)
                .and_then(|s| s.with(&meta, "waterlogged", "false").ok())
        } else {
            state_with(lv, "minecraft:air", &[])
        };
        if let Some(to) = to {
            lv.set_state(p, to, all());
        }
    }

    fn fill(&mut self, p: [i32; 3], fluid: Fluid) {
        let lv = &mut *self.0;
        let to = if property(lv, p, "waterlogged").is_some() && fluid == Fluid::Water {
            let meta = lv.host.block_states();
            lv.block_state(p)
                .and_then(|s| s.with(&meta, "waterlogged", "true").ok())
        } else {
            state_with(lv, fluid.id(), &[("level", "0")])
        };
        if let Some(to) = to {
            lv.set_state(p, to, all());
        }
    }

    fn effect(&mut self, p: [i32; 3], fluid: Fluid, fill: bool, fizz: bool) {
        // FluidHelper.getFillSound / getEmptySound, at 0.3 (FIRE_EXTINGUISH at 0.5 where
        // water boils away).
        let sound = match (fill, fluid == Fluid::Lava) {
            (true, false) => "minecraft:item.bucket.fill",
            (true, true) => "minecraft:item.bucket.fill_lava",
            (false, false) => "minecraft:item.bucket.empty",
            (false, true) => "minecraft:item.bucket.empty_lava",
        };
        let lv = &mut *self.0;
        let at = p.map(f64::from);
        lv.level
            .play_sound(&mut *lv.host, sound, at, SoundSource::Blocks, 0.3, 1.0);
        if fizz {
            lv.level.play_sound(
                &mut *lv.host,
                "minecraft:block.fire.extinguish",
                at,
                SoundSource::Blocks,
                0.5,
                2.6,
            );
        }
    }
}

/// After the pipes: all valves (`FluidValveBlockEntity.tick`), hose pulleys
/// given a speed or busy, whistles and their extensions, and what clients must see again.
/// Nothing runs without any.
pub(super) fn tick(lv: &mut ServerLevel<'_>) {
    tick_hoses(lv);
    tick_whistles(lv);
    send_changes(lv);
}

pub(crate) fn tick_entity(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let enabled = info_at(lv, pos)
        .filter(|(b, _)| *b == Block::FluidValve)
        .map(|(_, i)| i.enabled);
    if let Some(open) = with_world(|w| w.tick_fluid_entity(lv.dim, pos, enabled)).flatten() {
        crate::glue::items::wrench::switch_property(lv, pos, "enabled", flag(open));
    }
    super::pipes::tick_tank_states(lv);
}

fn tick_hoses(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let (speeds, busy) = with_world(|w| {
        let p = &mut w.dims[dim].plumbing;
        (
            std::mem::take(&mut p.hose_speeds),
            if p.hose_busy.is_empty() {
                Vec::new()
            } else {
                p.hose_busy.clone()
            },
        )
    })
    .unwrap_or_default();
    if speeds.is_empty() && busy.is_empty() {
        return;
    }
    let now = lv.game_time();
    for (pos, speed) in speeds {
        let Some(mut hose) = with_world(|w| w.dims[dim].plumbing.hoses.remove(&pos)).flatten()
        else {
            continue;
        };
        let level = HoseLevel(lv);
        if speed.is_nan() {
            hose.wake(&level, pos, now);
        } else {
            hose.speed_changed(&level, pos, speed, now);
        }
        settle_hose(dim, pos, hose, true);
    }
    for pos in busy {
        let Some(mut hose) = with_world(|w| w.dims[dim].plumbing.hoses.remove(&pos)).flatten()
        else {
            continue;
        };
        let (block, moving) = (hose.offset.ceil() as i32, hose.moving);
        hose.tick(&mut HoseLevel(lv), pos, now);
        let moved = hose.offset.ceil() as i32 != block || hose.moving != moving;
        with_world(|w| {
            if moved {
                w.mark_unsaved(dim, pos);
            }
            if !hose.busy() {
                w.dims[dim].plumbing.hose_busy.retain(|p| *p != pos);
            }
        });
        settle_hose(dim, pos, hose, moved);
    }
}

/// A hose back in place: clients see it again when it `changed`, it ticks while busy, and a
/// still one turning forwards waits for the block below its end (`lazyTick`).
fn settle_hose(dim: usize, pos: [i32; 3], hose: create_engine::hose::Hose, changed: bool) {
    with_world(|w| {
        let p = &mut w.dims[dim].plumbing;
        p.hose_waits.retain(|_, h| *h != pos);
        if !hose.moving && hose.movement() > 0.0 {
            let depth = (hose.offset + hose.movement()).ceil() as i32;
            p.hose_waits.insert([pos[0], pos[1] - depth, pos[2]], pos);
        }
        if changed {
            p.hose_changes.push(pos);
        }
        p.hoses.insert(pos, hose);
        p.hose_busy(pos);
    });
}

/// Creative tanks shown as full tanks of their fluid (`FluidTankRenderer`), hoses and smart
/// pipes' filters (`FILTER`: the slot's item, empty for none), to their block entities' clients
/// when they changed.
fn send_changes(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let filters = with_world(|w| {
        let p = &mut w.dims[dim].plumbing;
        let mut v = std::mem::take(&mut p.filter_changes);
        v.sort();
        v.dedup();
        v.into_iter()
            .map(|q| (q, p.filters.get(&q).map(|f| f.item.clone())))
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    for (pos, item) in filters {
        if !lv.is_loaded(pos) {
            continue;
        }
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let item = item.unwrap_or_default();
        let data = Compound::from([(FILTER.into(), Tag::String(item.as_str().into()))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
    let (tanks, hoses) = with_world(|w| {
        let p = &mut w.dims[dim].plumbing;
        let take = |v: &mut Vec<[i32; 3]>| {
            let mut v = std::mem::take(v);
            v.sort();
            v.dedup();
            v
        };
        (take(&mut p.creative_changes), take(&mut p.hose_changes))
    })
    .unwrap_or_default();
    // Each controller as a tank full of its fluid (`getFluidAmount` is its capacity), the other
    // blocks as no tank.
    for pos in tanks {
        let m = with_world(|w| w.dims[dim].creative_tanks.multis.get(&pos).copied())
            .flatten()
            .map(|m| Multi {
                tank: Tank {
                    fluid: m.tank.fluid,
                    amount: if m.tank.fluid.is_some() {
                        m.capacity()
                    } else {
                        0
                    },
                },
                ..m
            });
        super::set_shown_tank(lv, pos, m.as_ref());
    }
    let now = lv.game_time();
    for pos in hoses {
        let shown = with_world(|w| {
            let h = w.dims[dim].plumbing.hoses.get(&pos)?;
            Some((h.offset, if h.moving { h.movement() } else { 0.0 }))
        })
        .flatten();
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let v = shown.map_or_else(Vec::new, |(offset, per_tick)| {
            vec![
                Tag::Float(offset.to_bits()),
                Tag::Float(per_tick.to_bits()),
                Tag::Long(now),
            ]
        });
        let data = Compound::from([(HOSE.into(), Tag::List(v))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
}

/// `WrenchItem` on a fluid pipe, glass pipe or encased pipe (`onWrenched`); true when it
/// was one. A straight pipe (or, bent, the axis of the face centre nearest the click at
/// `point`, across the clicked face) becomes a glass pipe; a glass pipe a plain one again;
/// an encased pipe loses its casing. A wrench on the pipe's own axis passes.
pub(super) fn wrench_pipe(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    block: Block,
    clicked: Dir,
    point: [f64; 3],
) -> bool {
    let Some((_, info)) = info_at(lv, pos) else {
        return false;
    };
    let Some(state) = lv.block_state(pos) else {
        return false;
    };
    match block {
        Block::FluidPipe => {
            let sides = pipe_sides_of(lv, state);
            let axis = create_engine::fluids::straight_axis(sides).or_else(|| {
                let local = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]));
                let dist = |d: &Dir| {
                    let o = d.offset();
                    (0..3)
                        .map(|i| (local[i] - (0.5 + f64::from(o[i]) * 0.5)).powi(2))
                        .sum::<f64>()
                };
                Dir::ALL
                    .into_iter()
                    .filter(|d| d.axis() != clicked.axis())
                    .min_by(|a, b| dist(a).total_cmp(&dist(b)))
                    .map(|d| d.axis())
            });
            // `PASS`: nothing happens.
            let Some(axis) = axis.filter(|a| *a != clicked.axis()) else {
                return true;
            };
            let props = [
                ("axis", axis_name(axis)),
                ("waterlogged", flag(info.waterlogged)),
            ];
            if let Some(next) = state_with(lv, "create:glass_fluid_pipe", &props) {
                super::pipes::replace_pipe_state(lv, pos, next);
            }
            true
        }
        // AxisPipeBlock.toRegularPipe: open both ways along its axis, then connected as a pipe
        // (`updateBlockState`, preferring the axis' positive end), waterlogging kept.
        Block::GlassFluidPipe => {
            let Some(axis) = info.axis else {
                return true;
            };
            let props: Vec<(&str, &str)> = Dir::ALL
                .into_iter()
                .map(|d| (dir_name(d), flag(d.axis() == axis)))
                .chain([("waterlogged", flag(info.waterlogged))])
                .collect();
            if let Some(next) = state_with(lv, "create:fluid_pipe", &props) {
                super::pipes::replace_pipe_state(lv, pos, next);
                connect_pipe(lv, pos, Some(Dir::from_axis(axis, true)));
            }
            true
        }
        // EncasedPipeBlock.onWrenched: a pipe with its sides, connected again preferring the
        // first open side.
        Block::EncasedFluidPipe => {
            // `levelEvent(PARTICLES_DESTROY_BLOCK, pos, Block.getId(state))`: the encased
            // pipe's.
            if let Some(own) = lv.block_state(pos) {
                lv.destroy_effect(pos, own);
            }
            let sides = pipe_sides_of(lv, state);
            let first = Dir::ALL
                .into_iter()
                .find(|d| sides & d.bit() != 0)
                .unwrap_or(Dir::Up);
            let props: Vec<(&str, &str)> = Dir::ALL
                .into_iter()
                .map(|d| (dir_name(d), flag(sides & d.bit() != 0)))
                .collect();
            if let Some(next) = state_with(lv, "create:fluid_pipe", &props) {
                lv.set_state(pos, next, all());
                connect_pipe(lv, pos, Some(first));
            }
            true
        }
        _ => false,
    }
}

fn axis_name(a: Axis) -> &'static str {
    match a {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

/// `WrenchItem` on a whistle or extension: its size cycles
/// (`getRotatedBlockState`), an extension's whistle's; the extensions follow (`updateShape`).
/// True when it was one.
pub(super) fn wrench_whistle(lv: &mut ServerLevel<'_>, pos: [i32; 3], block: Block) -> bool {
    let root = match block {
        Block::SteamWhistle => Some(pos),
        Block::SteamWhistleExtension => whistle_root(lv, pos),
        _ => return false,
    };
    let Some(root) = root else {
        return true;
    };
    let Some(size) = info_at(lv, root).and_then(|(_, i)| i.size) else {
        return true;
    };
    lv.set_property(root, "size", size.cycled().name(), all());
    true
}

/// The stack of extensions on a whistle (`findRoot` from an extension): the first block below
/// that is no extension.
fn whistle_root(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<[i32; 3]> {
    let mut at = [pos[0], pos[1] - 1, pos[2]];
    for _ in 0..64 {
        match info_at(lv, at).map(|(b, _)| b) {
            Some(Block::SteamWhistleExtension) => at[1] -= 1,
            Some(Block::SteamWhistle) => return Some(at),
            _ => return None,
        }
    }
    None
}

/// An extension's shape at `height` above `root`, if one is there.
fn extension_above(
    lv: &ServerLevel<'_>,
    root: [i32; 3],
    height: i32,
) -> Option<create_engine::whistle::Shape> {
    match info_at(lv, [root[0], root[1] + height, root[2]]) {
        Some((Block::SteamWhistleExtension, i)) => i.extender,
        _ => None,
    }
}

fn tick_whistles(lv: &mut ServerLevel<'_>) {
    use create_engine::whistle;
    let dim = lv.dim;
    // Whistles and extensions that may no longer stand break with their drops (`updateShape`,
    // `canSurvive`); extensions join the one above (`double_connected`) or leave it.
    let (support, extensions) = with_world(|w| {
        let p = &mut w.dims[dim].plumbing;
        (
            std::mem::take(&mut p.whistle_support),
            std::mem::take(&mut p.extension_checks),
        )
    })
    .unwrap_or_default();
    for pos in support {
        let Some(Some(a)) = (match info_at(lv, pos) {
            Some((Block::SteamWhistle, i)) => Some(i.facing.map(|f| whistle::attached(i.wall, f))),
            _ => None,
        }) else {
            continue;
        };
        let held = matches!(
            info_at(lv, offset(pos, a)).map(|(b, _)| b),
            Some(Block::FluidTank | Block::CreativeFluidTank)
        );
        if !held {
            lv.destroy_block(pos, true);
        }
    }
    let mut roots = Vec::new();
    for pos in extensions {
        let Some(shape) = (match info_at(lv, pos) {
            Some((Block::SteamWhistleExtension, i)) => i.extender,
            _ => None,
        }) else {
            // Gone: the whistle below counts its pitch again.
            if let Some(root) = whistle_root(lv, pos) {
                roots.push(root);
            }
            continue;
        };
        let (below_whistle, below_ext, below_size) = match info_at(lv, [pos[0], pos[1] - 1, pos[2]])
        {
            Some((Block::SteamWhistle, i)) => (true, None, i.size),
            Some((Block::SteamWhistleExtension, i)) => (false, i.extender, i.size),
            _ => (false, None, None),
        };
        if !whistle::extension_survives(below_whistle, below_ext) {
            lv.destroy_block(pos, true);
            continue;
        }
        let above = extension_above(lv, pos, 1).is_some();
        let want = whistle::connected(shape, above);
        if want != shape {
            lv.set_property(pos, "shape", want.name(), all());
        }
        if let Some(size) = below_size {
            lv.set_property(pos, "size", size.name(), all());
        }
        if let Some(root) = whistle_root(lv, pos) {
            roots.push(root);
        }
    }
    with_world(|w| w.dims[dim].plumbing.whistle_checks.extend(roots));
    // `updatePitch`.
    for pos in with_world(|w| w.take_whistle_checks(dim)).unwrap_or_default() {
        let pitch = whistle::pitch(|h| extension_above(lv, pos, h));
        let changed = with_world(|w| {
            let p = &mut w.dims[dim].plumbing;
            if let Some(wh) = p.whistles.get_mut(&pos)
                && wh.pitch != pitch
            {
                wh.pitch = pitch;
                // A sounding whistle changes its note at once.
                wh.replay_in = 0;
                p.sound_checks.push(pos);
                return true;
            }
            false
        });
        if changed == Some(true) {
            crate::glue::advancements::pipe_organ(lv, pos);
        }
    }
    // Whose sound may have started or stopped, and those sounding (`tickAudio`).
    let mut look = with_world(|w| w.take_sound_checks(dim)).unwrap_or_default();
    look.extend(with_world(|w| w.dims[dim].plumbing.whistling.clone()).unwrap_or_default());
    look.sort();
    look.dedup();
    for pos in look {
        let Some((powered, size)) = (match info_at(lv, pos) {
            Some((Block::SteamWhistle, i)) => Some((i.powered, i.size.unwrap_or_default())),
            _ => None,
        }) else {
            continue;
        };
        if powered {
            crate::glue::advancements::award_placer(lv, pos, "steam_whistle");
        }
        let Some((sounds, mut wh)) = with_world(|w| {
            let sounds = whistle::sounds(powered, w.whistle_boiler(dim, pos));
            Some((sounds, *w.dims[dim].plumbing.whistles.get(&pos)?))
        })
        .flatten() else {
            continue;
        };
        let at = pos.map(|v| f64::from(v) + 0.5);
        if sounds {
            if !wh.sounding {
                // WHISTLE_CHIFF as it starts, `maxVolume * .175`.
                let p = whistle::chiff_pitch(size, wh.pitch);
                lv.level.play_sound(
                    &mut *lv.host,
                    "create:chiff",
                    at,
                    SoundSource::Records,
                    0.175,
                    p,
                );
            }
            if wh.replay_in == 0 {
                let p = whistle::sound_pitch(wh.pitch);
                lv.level.play_sound(
                    &mut *lv.host,
                    size.sound(),
                    at,
                    SoundSource::Records,
                    1.0,
                    p,
                );
                wh.replay_in = whistle::replay_ticks(wh.pitch);
            } else {
                wh.replay_in -= 1;
            }
            wh.sounding = true;
        } else if wh.sounding {
            // `fadeOut`: the loop stops (every whistle of that sound; the others start theirs
            // again at once).
            lv.host
                .commands()
                .run_command(&format!("stopsound @a record {}", size.sound()));
            wh.sounding = false;
            wh.replay_in = 0;
            with_world(|w| {
                let p = &mut w.dims[dim].plumbing;
                for o in p.whistling.clone() {
                    if let Some(other) = p.whistles.get_mut(&o) {
                        other.replay_in = 0;
                    }
                }
            });
        }
        let was = with_world(|w| {
            let p = &mut w.dims[dim].plumbing;
            let was = p.whistles.insert(pos, wh).is_some_and(|o| o.sounding);
            p.whistling.retain(|q| *q != pos);
            if wh.sounding {
                p.whistling.push(pos);
            }
            was
        })
        .unwrap_or(false);
        if was != wh.sounding
            && let Some(entity) = lv.host.block_entity(lv.level, pos)
        {
            // `WhistleBlockEntity.tick`'s `powered` on its clients, and since when.
            let v = vec![i32::from(wh.sounding), lv.game_time() as i32];
            let data = Compound::from([(WHISTLE.into(), Tag::IntArray(v))]);
            let _ = entity.merge_data(&mut *lv.host, data);
        }
    }
}

/// The key of whether a whistle blows, and the game time it started or stopped, in its block
/// entity's data for its clients (`WhistleRenderer`'s mouth).
pub const WHISTLE: &str = "Whistle";

/// The key of a smart pipe's filter item in its block entity's data, for its clients.
pub const FILTER: &str = "PipeFilter";

/// Smart pipe filters (x, y, z, fluid; 255: none, and their items under `filter_items`), hoses (x, y, z, offset × 1000, fluid, amount) and
/// creative tanks (x, y, z, fluid; 255: none) saved with a column, under `create_fluids`.
const KEY: &str = "create_fluids";

pub(super) fn chunk_data_save(dim: usize, chunk: [i32; 2], data: &mut Compound) {
    let here = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]);
    let codes = std::cell::RefCell::new(create_engine::tank::Codes::default());
    let index = |f: Option<Fluid>| codes.borrow_mut().code(f);
    let (filters, hoses, creative, multis) = with_world(|w| {
        let p = &w.dims[dim].plumbing;
        let mut filters: Vec<_> = p
            .filters
            .iter()
            .filter(|(q, _)| here(q))
            .map(|(q, f)| (*q, f.clone()))
            .collect();
        filters.sort_by_key(|e| e.0);
        let mut hoses: Vec<_> = p
            .hoses
            .iter()
            .filter(|(q, _)| here(q))
            .map(|(q, h)| (*q, h.offset, h.tank))
            .chain(
                p.hose_saved
                    .iter()
                    .filter(|(q, _)| here(q))
                    .map(|(q, s)| (*q, s.0, s.1)),
            )
            .collect();
        hoses.sort_by_key(|e| e.0);
        let mut creative: Vec<_> = p
            .creative
            .iter()
            .chain(p.creative_saved.iter())
            .filter(|(q, _)| here(q))
            .map(|(q, f)| (*q, *f))
            .collect();
        creative.sort_by_key(|e| e.0);
        let multis = w.column_creative_tanks(dim, chunk[0], chunk[1]);
        (filters, hoses, creative, multis)
    })
    .unwrap_or_default();
    if filters.is_empty() && hoses.is_empty() && creative.is_empty() {
        return;
    }
    let mut own = Compound::new();
    let v: Vec<i32> = filters
        .iter()
        .flat_map(|(p, f)| [p[0], p[1], p[2], index(f.fluid)])
        .collect();
    own.insert("filters".into(), Tag::IntArray(v));
    // The items in the filters' slots, in the same order.
    let items: Vec<Tag> = filters
        .iter()
        .map(|(_, f)| Tag::String(f.item.as_str().into()))
        .collect();
    own.insert("filter_items".into(), Tag::List(items));
    let v: Vec<i32> = hoses
        .iter()
        .flat_map(|(p, o, t)| {
            [
                p[0],
                p[1],
                p[2],
                (o * 1000.0) as i32,
                index(t.fluid),
                t.amount as i32,
            ]
        })
        .collect();
    own.insert("hoses".into(), Tag::IntArray(v));
    let v: Vec<i32> = creative
        .iter()
        .flat_map(|(p, f)| [p[0], p[1], p[2], index(*f)])
        .collect();
    own.insert("creative_tanks".into(), Tag::IntArray(v));
    // x, y, z, width, height, window of each creative tank bigger than a block or without
    // windows (`Size`, `Height`, `Window`).
    let v: Vec<i32> = multis
        .iter()
        .flat_map(|(p, m)| {
            [
                p[0],
                p[1],
                p[2],
                i32::from(m.width),
                i32::from(m.height),
                i32::from(m.window),
            ]
        })
        .collect();
    own.insert("creative_multis".into(), Tag::IntArray(v));
    super::put_fluid_keys(&mut own, KEY, codes.into_inner());
    data.insert(KEY.into(), Tag::Compound(own));
}

/// What a column saved, before its blocks are found (they take it back when they are).
pub(super) fn chunk_load(dim: usize, data: &Compound) {
    let Some(Tag::Compound(own)) = data.get(&KEY.into()) else {
        return;
    };
    let ints = |k: &str| match own.get(&k.into()) {
        Some(Tag::IntArray(v)) => v.clone(),
        _ => Vec::new(),
    };
    let keys = super::fluid_keys(own, KEY);
    let fluid = |i: i32| create_engine::tank::Codes::fluid(&keys, i);
    let filters = ints("filters");
    let filter_items: Vec<String> = match own.get(&"filter_items".into()) {
        Some(Tag::List(l)) => l
            .iter()
            .map(|t| match t {
                Tag::String(s) => s.to_string_lossy(),
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    };
    let hoses = ints("hoses");
    let creative = ints("creative_tanks");
    let multis = ints("creative_multis");
    with_world(|w| {
        let p = &mut w.dims[dim].plumbing;
        for (i, c) in filters.as_chunks::<4>().0.iter().enumerate() {
            let fluid = fluid(c[3]);
            // Saved before the slot's item was: its fluid's bucket.
            let item = filter_items
                .get(i)
                .cloned()
                .unwrap_or_else(|| fluid.and_then(Fluid::bucket).unwrap_or_default().to_owned());
            let pos = [c[0], c[1], c[2]];
            p.filters.insert(pos, SmartFilter { item, fluid });
            p.filter_changes.push(pos);
        }
        for c in hoses.as_chunks::<6>().0 {
            let tank = Tank {
                fluid: fluid(c[4]),
                amount: c[5].clamp(0, create_engine::hose::CAPACITY as i32) as u32,
            };
            p.hose_saved
                .insert([c[0], c[1], c[2]], ((c[3].max(0) as f32) / 1000.0, tank));
        }
        for c in creative.as_chunks::<4>().0 {
            p.creative_saved.insert([c[0], c[1], c[2]], fluid(c[3]));
        }
        let multis = multis.as_chunks::<6>().0.iter().map(|c| {
            let m = Multi {
                width: c[3].clamp(1, MAX_WIDTH) as u8,
                height: c[4].clamp(1, MAX_HEIGHT) as u8,
                window: c[5] != 0,
                tank: Tank::default(),
            };
            ([c[0], c[1], c[2]], m)
        });
        w.load_creative_tanks(dim, multis);
    });
}

/// `WhistleBlock.useItemOn` with a whistle in hand: the stack on the whistle (or the clicked
/// extension's whistle) grows by an extension, or its top one doubles (`incrementSize`), with
/// copper's sounds at the new piece's note. True when it was a whistle's stack.
pub(super) fn grow_whistle_at(lv: &mut ServerLevel<'_>, pos: [i32; 3], block: Block) -> bool {
    use create_engine::whistle::Grow;
    let root = match block {
        Block::SteamWhistle => Some(pos),
        _ => whistle_root(lv, pos),
    };
    let Some(root) = root else {
        return true;
    };
    let Some(size) = info_at(lv, root).and_then(|(_, i)| i.size) else {
        return true;
    };
    let grow = create_engine::whistle::grow(
        |h| extension_above(lv, root, h),
        |h| super::replaceable(lv, [root[0], root[1] + h, root[2]]),
    );
    let (height, pitch) = match grow {
        Some(Grow::Double(h, p)) => {
            lv.set_property([root[0], root[1] + h, root[2]], "shape", "double", all());
            (h, p)
        }
        Some(Grow::Place(h, p)) => {
            let props = [("shape", "single"), ("size", size.name())];
            if let Some(s) = state_with(lv, "create:steam_whistle_extension", &props) {
                lv.set_state([root[0], root[1] + h, root[2]], s, all());
            }
            (h, p)
        }
        None => return true,
    };
    let at = [root[0], root[1] + height, root[2]].map(f64::from);
    lv.level.play_sound(
        &mut *lv.host,
        "minecraft:block.note_block.xylophone",
        at,
        SoundSource::Blocks,
        0.25,
        pitch,
    );
    lv.level.play_sound(
        &mut *lv.host,
        "minecraft:block.copper.hit",
        at,
        SoundSource::Blocks,
        1.0,
        pitch,
    );
    true
}
