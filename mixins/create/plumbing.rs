// Create's fluid valves, smart, glass and encased pipes, hose pulleys, creative fluid tanks and
// steam whistles in the game. A sibling of the `create` module (`runtime.rs`); the rules are
// `create_engine::{fluids, hose, whistle, plumbing}`.
use super::Game;
use super::create::*;
use super::create_fluids::*;
use super::create_logistics::*;
use create_engine::Block;
use create_engine::fluids::{Cell, Gate};
use create_engine::kinetics::Dir;
use create_engine::parts::StateInfo;
use create_engine::tank::Fluid;
use create_engine::world::dimension_index;
use horizon_world::registry::BlockStateId;

const ALL: i32 = super::native_hooks::flags::UPDATE_ALL;

fn offset(p: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

fn flag(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// What a valve, a smart, glass or encased pipe, a hose pulley or a creative tank is to the
/// pipes; None for other blocks.
pub(super) fn plumbing_cell(
    game: &Game<'_>,
    state: BlockStateId,
    block: Block,
    info: &StateInfo,
    pos: [i32; 3],
) -> Option<Cell> {
    use create_engine::fluids::{smart_pipe_axis, valve_pipe_axis};
    Some(match block {
        Block::EncasedFluidPipe => Cell::Encased(pipe_sides_of(game, state)),
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
            let filter = dimension_index(game.dim.id())
                .and_then(|dim| with_world(|w| w.dims[dim].plumbing.filters.get(&pos).copied()))
                .flatten();
            Cell::Axis(
                smart_pipe_axis(wall, info.facing?),
                filter.map_or(Gate::Open, Gate::Only),
            )
        }
        Block::HosePulley => Cell::Port(create_engine::hose::pipe_side(info.facing?)),
        Block::CreativeFluidTank => Cell::Tank,
        _ => return None,
    })
}

/// A pump's source end that is a creative tank or a hose pulley: what it gives this tick (a
/// hose simulates its pull). None for other ends.
pub(super) fn source(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    amount: u32,
) -> Option<Option<(Fluid, u32)>> {
    // CreativeSmartFluidTank.drain: always full of its fluid, never emptied.
    if let Some(fluid) = with_world(|w| w.dims[dim].plumbing.creative_fluid(pos)).flatten() {
        return Some(fluid.map(|f| (f, amount)));
    }
    let mut hose = with_world(|w| w.dims[dim].plumbing.hoses.remove(&pos)).flatten()?;
    let now = game.world.game_time;
    let out = hose.drain(&mut HoseLevel::new(game, dimension), pos, amount, true, now);
    put_hose(dim, pos, hose);
    Some(out)
}

/// What came out of a creative tank or hose pulley the pump drew from (the hose pulls for
/// real).
pub(super) fn take(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3], moved: u32) {
    let Some(mut hose) = with_world(|w| w.dims[dim].plumbing.hoses.remove(&pos)).flatten() else {
        return;
    };
    let now = game.world.game_time;
    hose.drain(&mut HoseLevel::new(game, dimension), pos, moved, false, now);
    put_hose(dim, pos, hose);
    with_world(|w| w.mark_unsaved(dim, pos));
}

/// A pump's end that is a creative tank (it takes everything, `CreativeSmartFluidTank.fill`)
/// or a hose pulley (`HosePulleyFluidHandler.fill`): what it took. None for other ends.
pub(super) fn sink(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    fluid: Fluid,
    amount: u32,
) -> Option<u32> {
    if with_world(|w| w.dims[dim].plumbing.creative.contains_key(&pos)) == Some(true) {
        return Some(amount);
    }
    let mut hose = with_world(|w| w.dims[dim].plumbing.hoses.remove(&pos)).flatten()?;
    let now = game.world.game_time;
    let hot = dimension == "minecraft:the_nether";
    let took = hose.fill(
        &mut HoseLevel::new(game, dimension),
        pos,
        fluid,
        amount,
        hot,
        now,
    );
    put_hose(dim, pos, hose);
    if took > 0 {
        with_world(|w| w.mark_unsaved(dim, pos));
    }
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
struct HoseLevel<'a, 'g> {
    game: &'a mut Game<'g>,
    dimension: &'a str,
}

impl<'a, 'g> HoseLevel<'a, 'g> {
    fn new(game: &'a mut Game<'g>, dimension: &'a str) -> Self {
        HoseLevel { game, dimension }
    }

    fn set(&mut self, p: [i32; 3], state: BlockStateId) {
        self.game.native_set_block(self.dimension, p, state, ALL);
    }
}

impl create_engine::hose::Level for HoseLevel<'_, '_> {
    fn loaded(&self, p: [i32; 3]) -> bool {
        self.game.world.loaded(p[0], p[2])
    }

    fn space(&self, p: [i32; 3]) -> create_engine::hose::Space {
        use create_engine::hose::Space;
        use horizon_world::fluid::FluidKind;
        let state = self.game.world.block(p[0], p[1], p[2]);
        let b = self.game.res.behaviours();
        if b.is_air(state) {
            return Space::Replaceable;
        }
        let f = b.fluid(state);
        let fluid = match f.kind() {
            FluidKind::Water => Some(Fluid::Water),
            FluidKind::Lava => Some(Fluid::Lava),
            _ => None,
        };
        let props = self.game.pack.assets.registry.state(state);
        if let Some(w) = props.and_then(|s| s.properties.get("waterlogged")) {
            return Space::Waterloggable(w == "true");
        }
        // A fluid's own block (`LiquidBlock`: its `level`).
        if let Some(fluid) = fluid
            && props.is_some_and(|s| s.properties.contains_key("level"))
        {
            return Space::Liquid(fluid, f.is_source());
        }
        let open =
            b.facts(state).flags & horizon_world::level::data::state_flags::COLLISION_EMPTY != 0;
        if let Some(fluid) = fluid {
            return if open {
                Space::Holding(fluid)
            } else {
                Space::Solid
            };
        }
        // `canBeReplacedByFluid`: what doesn't block motion, but signs, ladders, sugar cane,
        // bubble columns, portals and structure voids.
        let kept = props.is_some_and(|s| {
            matches!(
                s.name.as_str(),
                "minecraft:sugar_cane"
                    | "minecraft:nether_portal"
                    | "minecraft:end_portal"
                    | "minecraft:end_gateway"
                    | "minecraft:structure_void"
            )
        });
        if open && !kept {
            Space::Replaceable
        } else {
            Space::Solid
        }
    }

    fn passable(&self, p: [i32; 3]) -> bool {
        let state = self.game.world.block(p[0], p[1], p[2]);
        self.game.res.behaviours().is_replaceable(state)
    }

    fn empty(&mut self, p: [i32; 3]) {
        let state = self.game.world.block(p[0], p[1], p[2]);
        let registry = self.game.pack.assets.registry;
        let to = match registry.state(state) {
            Some(s) if s.properties.contains_key("waterlogged") => {
                let mut s = s.clone();
                s.properties.insert("waterlogged".into(), "false".into());
                registry.id(&s)
            }
            _ => registry.default_state("minecraft:air"),
        };
        if let Some(to) = to {
            self.set(p, to);
        }
    }

    fn fill(&mut self, p: [i32; 3], fluid: Fluid) {
        let state = self.game.world.block(p[0], p[1], p[2]);
        let registry = self.game.pack.assets.registry;
        let to = match registry.state(state) {
            Some(s) if s.properties.contains_key("waterlogged") && fluid == Fluid::Water => {
                let mut s = s.clone();
                s.properties.insert("waterlogged".into(), "true".into());
                registry.id(&s)
            }
            _ => state_with(registry, fluid.id(), &[("level", "0")]),
        };
        if let Some(to) = to {
            self.set(p, to);
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
        let (x, y, z) = (p[0], p[1], p[2]);
        let d = self.dimension;
        self.game.native_run_command(&format!(
            "execute in {d} run playsound {sound} blocks @a {x} {y} {z} 0.3 1"
        ));
        if fizz {
            self.game.native_run_command(&format!(
                "execute in {d} run playsound minecraft:block.fire.extinguish blocks @a {x} {y} {z} 0.5 2.6"
            ));
        }
    }
}

/// After the level's tick: valves whose pointer moves (`FluidValveBlockEntity.tick`), hose
/// pulleys given a speed or busy, whistles and their extensions. Nothing runs without any.
pub(super) fn tick_plumbing(game: &mut Game<'_>, dimension: &str, dim: usize) {
    tick_valves(game, dim);
    tick_hoses(game, dimension, dim);
    tick_whistles(game, dimension, dim);
    send_plumbing_changes(game, dimension, dim);
}

fn tick_valves(game: &mut Game<'_>, dim: usize) {
    let changes = with_world(|w| {
        let table = &w.table;
        let world = &game.world;
        w.dims[dim].plumbing.tick_valves(|p| {
            let s = world.block(p[0], p[1], p[2]);
            table
                .get(s.0)
                .filter(|(b, _)| *b == Block::FluidValve)
                .map(|(_, i)| i.enabled)
        })
    })
    .unwrap_or_default();
    for (pos, open) in changes {
        // `switchToBlockState`: the pipes see it open or shut (`propagateChangedPipe`).
        game.native_set_block_property_with(pos, "enabled", flag(open), ALL);
    }
}

fn tick_hoses(game: &mut Game<'_>, dimension: &str, dim: usize) {
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
    let now = game.world.game_time;
    for (pos, speed) in speeds {
        let Some(mut hose) = with_world(|w| w.dims[dim].plumbing.hoses.remove(&pos)).flatten()
        else {
            continue;
        };
        let level = HoseLevel::new(game, dimension);
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
        hose.tick(&mut HoseLevel::new(game, dimension), pos, now);
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

/// `HosePulleyBlock.getStateForPlacement`.
pub(super) fn place_hose(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    let horizontal = create_engine::place::horizontal_direction(game.yaw);
    let pipes: Vec<Dir> = Dir::ALL
        .into_iter()
        .filter(|d| create_engine::fluids::connects(pipe_cell(game, offset(pos, *d)), *d))
        .collect();
    let facing = with_world(|w| {
        create_engine::hose::placement(
            |side| w.has_shaft_towards(dim, offset(pos, side), side.opposite()),
            |side| pipes.contains(&side),
            horizontal,
        )
    })
    .unwrap_or(horizontal.opposite());
    game.native_set_block_property(pos, "facing", dir_name(facing));
}

/// `FluidValveBlock` placed (`DirectionalAxisKineticBlock.getStateForPlacement`).
pub(super) fn place_valve(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    let nearest = create_engine::place::ordered_by_nearest(game.yaw, game.pitch)[0];
    let horizontal = create_engine::place::horizontal_direction(game.yaw);
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let pipes: Vec<Dir> = Dir::ALL
        .into_iter()
        .filter(|d| create_engine::fluids::connects(pipe_cell(game, offset(pos, *d)), *d))
        .collect();
    let Some((facing, along)) = with_world(|w| {
        create_engine::fluids::valve_placement(
            nearest,
            sneaking,
            horizontal,
            |side| w.has_shaft_towards(dim, offset(pos, side), side.opposite()),
            |side| pipes.contains(&side),
        )
    }) else {
        return;
    };
    game.native_set_block_property(pos, "facing", dir_name(facing));
    game.native_set_block_property(pos, "axis_along_first", flag(along));
}

/// `SmartFluidPipeBlock.getStateForPlacement`: on the face looked at
/// (`FaceAttachedHorizontalDirectionalBlock`), then turned to the pipes around it.
pub(super) fn place_smart_pipe(game: &mut Game<'_>, pos: [i32; 3]) {
    use create_engine::steam::Face;
    let looking = create_engine::place::ordered_by_nearest(game.yaw, game.pitch);
    let horizontal = create_engine::place::horizontal_direction(game.yaw);
    let (mut face, mut facing) = match looking[0] {
        Dir::Up => (Face::Ceiling, horizontal),
        Dir::Down => (Face::Floor, horizontal),
        d => (Face::Wall, d.opposite()),
    };
    let pipes: Vec<Dir> = Dir::ALL
        .into_iter()
        .filter(|d| create_engine::fluids::connects(pipe_cell(game, offset(pos, *d)), *d))
        .collect();
    if let Some((f, d)) =
        create_engine::fluids::smart_pipe_placement(|d| pipes.contains(&d), looking, face, facing)
    {
        (face, facing) = (f, d);
    }
    game.native_set_block_property(pos, "face", face.name());
    game.native_set_block_property(pos, "facing", dir_name(facing));
}

/// A right click on a valve, smart pipe, glass pipe, encased pipe, creative tank, whistle or
/// extension (their `useItemOn`): true when it was one of them.
pub(super) fn use_plumbing(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    block: Block,
    canceled: &mut bool,
) -> bool {
    let held = game.native_held().map(|h| h.0);
    let held = held.as_deref();
    match block {
        // EncasableBlock.tryEncase / AxisPipeBlock.useItemOn: copper casing on a pipe encases
        // it, open where the pipe was (a glass pipe along its axis); the casing is kept.
        Block::FluidPipe | Block::GlassFluidPipe if held == Some("create:copper_casing") => {
            *canceled = true;
            encase(game, pos, block);
        }
        Block::SmartFluidPipe => use_smart_pipe(game, dim, pos, held, canceled),
        Block::CreativeFluidTank => use_creative_tank(game, dim, pos, held, canceled),
        // WhistleBlock.useItemOn: a whistle in hand grows the stack (`incrementSize`); on an
        // extension, its whistle's.
        Block::SteamWhistle | Block::SteamWhistleExtension
            if held == Some("create:steam_whistle") =>
        {
            *canceled = true;
            let root = if block == Block::SteamWhistle {
                Some(pos)
            } else {
                whistle_root(game, pos)
            };
            if let Some(root) = root {
                grow_whistle(game, root);
            }
        }
        _ => return false,
    }
    true
}

/// `EncasedPipeBlock.handleEncasing`, `AxisPipeBlock.useItemOn`.
fn encase(game: &mut Game<'_>, pos: [i32; 3], block: Block) {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let sides = if block == Block::FluidPipe {
        pipe_sides_of(game, state)
    } else {
        let axis = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.axis)).flatten();
        axis.map_or(0, |a| {
            Dir::from_axis(a, true).bit() | Dir::from_axis(a, false).bit()
        })
    };
    let registry = game.pack.assets.registry;
    let props: Vec<(&str, &str)> = Dir::ALL
        .into_iter()
        .map(|d| (dir_name(d), flag(sides & d.bit() != 0)))
        .collect();
    let Some(id) = state_with(registry, "create:encased_fluid_pipe", &props) else {
        return;
    };
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, id, ALL);
    // `playEncaseSound`: the casing's place sound.
    let (x, y, z) = (pos[0], pos[1], pos[2]);
    game.native_run_command(&format!(
        "execute in {dimension} run playsound minecraft:block.copper.place blocks @a {x} {y} {z} 1 0.8"
    ));
}

/// The smart pipe's filter (`FilteringBehaviour.forFluids`, set as Create's value box sets it):
/// a filled bucket sets it to its fluid, an empty hand clears it; the pipes look again
/// (`onFilterChanged`: `propagateChangedPipe`). A wrench or sneaking passes.
fn use_smart_pipe(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    held: Option<&str>,
    canceled: &mut bool,
) {
    if !may_use_value_box(game) || held == Some("create:wrench") {
        return;
    }
    let fluid = held.and_then(Fluid::of_bucket);
    if held.is_some() && fluid.is_none() {
        return;
    }
    *canceled = true;
    with_world(|w| {
        let d = &mut w.dims[dim];
        let had = match fluid {
            Some(f) => d.plumbing.filters.insert(pos, f) != Some(f),
            None => d.plumbing.filters.remove(&pos).is_some(),
        };
        if had {
            d.pipes_changed = true;
            d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        }
    });
    let center = pos.map(|v| f64::from(v) + 0.5);
    game.audio.play(
        "minecraft:entity.item_frame.add_item",
        Some(center),
        0.25,
        0.1,
    );
}

/// `FluidTankBlock.useItemOn` on a creative tank (anyone): a filled bucket sets the fluid it
/// gives without end, kept (`setContainedFluid`); an empty bucket is filled from it, which
/// clears it.
fn use_creative_tank(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    held: Option<&str>,
    canceled: &mut bool,
) {
    let Some(held) = held else {
        return;
    };
    let now = with_world(|w| w.dims[dim].plumbing.creative_fluid(pos))
        .flatten()
        .flatten();
    let next = if let Some(f) = Fluid::of_bucket(held) {
        // `tryEmptyItemIntoBE`: a survival player's bucket is emptied.
        if !game.creative {
            game.native_take_items(held, 1);
            game.native_give_items("minecraft:bucket", 1);
        }
        Some(f)
    } else if held == "minecraft:bucket" {
        let Some(f) = now else {
            return;
        };
        if !game.creative {
            game.native_take_items("minecraft:bucket", 1);
            game.native_give_items(f.bucket(), 1);
        } else if game.native_count_items(f.bucket()) == 0 {
            game.native_give_items(f.bucket(), 1);
        }
        None
    } else {
        return;
    };
    *canceled = true;
    if next != now {
        with_world(|w| {
            let p = &mut w.dims[dim].plumbing;
            p.creative.insert(pos, next);
            p.creative_changes.push(pos);
            w.mark_unsaved(dim, pos);
        });
        analog_touched(dim, pos);
    }
}

/// `WrenchItem` on a fluid pipe, glass pipe or encased pipe (`onWrenched`); true when done.
/// A straight pipe (or, bent, the axis nearest where it was clicked, across the face) becomes a
/// glass pipe; a glass pipe a plain one again; an encased pipe loses its casing. A wrench on the
/// pipe's own axis passes.
pub(super) fn wrench_pipe(game: &mut Game<'_>, pos: [i32; 3], block: Block, clicked: Dir) -> bool {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let registry = game.pack.assets.registry;
    let info = with_world(|w| w.table.get(state.0).map(|(_, i)| *i)).flatten();
    let Some(info) = info else {
        return false;
    };
    let dimension = game.dim.id().to_owned();
    match block {
        Block::FluidPipe => {
            let sides = pipe_sides_of(game, state);
            let axis = create_engine::fluids::straight_axis(sides).or_else(|| {
                // The nearest face's centre to the click, off the clicked face's axis.
                let hit = game.pick()?;
                let local = [0, 1, 2].map(|i| hit.point[i] - f64::from(pos[i]));
                Dir::ALL
                    .into_iter()
                    .filter(|d| d.axis() != clicked.axis())
                    .min_by(|a, b| {
                        let dist = |d: &Dir| {
                            let o = d.offset();
                            (0..3)
                                .map(|i| (local[i] - (0.5 + f64::from(o[i]) * 0.5)).powi(2))
                                .sum::<f64>()
                        };
                        dist(a).total_cmp(&dist(b))
                    })
                    .map(|d| d.axis())
            });
            // `PASS`: nothing happens.
            let Some(axis) = axis.filter(|a| *a != clicked.axis()) else {
                return true;
            };
            let a = ["x", "y", "z"][axis_index(axis) as usize];
            let water = flag(info.waterlogged);
            if let Some(id) = state_with(
                registry,
                "create:glass_fluid_pipe",
                &[("axis", a), ("waterlogged", water)],
            ) {
                game.native_set_block(&dimension, pos, id, ALL);
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
            if let Some(id) = state_with(registry, "create:fluid_pipe", &props) {
                game.native_set_block(&dimension, pos, id, ALL);
                connect_pipe(game, &dimension, pos, Some(Dir::from_axis(axis, true)));
            }
            true
        }
        // EncasedPipeBlock.onWrenched: a pipe with its sides, connected again preferring the
        // first open side.
        Block::EncasedFluidPipe => {
            let sides = pipe_sides_of(game, state);
            let first = Dir::ALL
                .into_iter()
                .find(|d| sides & d.bit() != 0)
                .unwrap_or(Dir::Up);
            let props: Vec<(&str, &str)> = Dir::ALL
                .into_iter()
                .map(|d| (dir_name(d), flag(sides & d.bit() != 0)))
                .collect();
            if let Some(id) = state_with(registry, "create:fluid_pipe", &props) {
                game.native_set_block(&dimension, pos, id, ALL);
                connect_pipe(game, &dimension, pos, Some(first));
            }
            true
        }
        _ => false,
    }
}

/// Creative tanks drawn as full tanks of their fluid (`FluidTankRenderer`), and hoses, to the
/// level's players when they changed.
fn send_plumbing_changes(game: &mut Game<'_>, dimension: &str, dim: usize) {
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
    if tanks.is_empty() && hoses.is_empty() {
        return;
    }
    let players = players_in(game, dimension);
    send_creative_tanks(game, &players, dim, &tanks);
    send_hoses(game, &players, dim, &hoses);
}

pub(super) fn send_creative_tanks(
    game: &mut Game<'_>,
    players: &[u128],
    dim: usize,
    tanks: &[[i32; 3]],
) {
    use create_engine::tank::{CAPACITY, Multi, Tank};
    if tanks.is_empty() {
        return;
    }
    let all: Vec<([i32; 3], Option<Multi>)> = with_world(|w| {
        tanks
            .iter()
            .map(|p| {
                let f = w.dims[dim].plumbing.creative_fluid(*p).flatten();
                let m = f.map(|f| Multi {
                    tank: Tank {
                        fluid: Some(f),
                        amount: CAPACITY,
                    },
                    width: 1,
                    height: 1,
                    window: true,
                });
                (*p, m)
            })
            .collect()
    })
    .unwrap_or_default();
    for m in create_engine::tank::encode(&all) {
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::tank::CHANNEL, &m);
        }
    }
}

pub(super) fn send_hoses(game: &mut Game<'_>, players: &[u128], dim: usize, hoses: &[[i32; 3]]) {
    if hoses.is_empty() {
        return;
    }
    let all: Vec<create_engine::hose::Record> = with_world(|w| {
        hoses
            .iter()
            .map(|p| {
                let h = w.dims[dim].plumbing.hoses.get(p);
                let v = h.map(|h| (h.offset, if h.moving { h.movement() } else { 0.0 }));
                (*p, v)
            })
            .collect()
    })
    .unwrap_or_default();
    for m in create_engine::hose::encode(&all) {
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::hose::CHANNEL, &m);
        }
    }
}

/// A joining player sees every creative tank and hose of their dimension.
pub(super) fn sync_plumbing(game: &mut Game<'_>, player: u128, dim: usize) {
    let (tanks, hoses) = with_world(|w| {
        let p = &w.dims[dim].plumbing;
        (
            p.creative.keys().copied().collect::<Vec<_>>(),
            p.hoses.keys().copied().collect::<Vec<_>>(),
        )
    })
    .unwrap_or_default();
    send_creative_tanks(game, &[player], dim, &tanks);
    send_hoses(game, &[player], dim, &hoses);
}

/// The hose of a pulley on the client (`AbstractPulleyRenderer`): the magnet (or its half near
/// the top) at the hose's end, gliding while it moves, and the hose's pieces down to it.
pub(super) fn hoses_from_server(game: &mut Game<'_>, data: &[u8]) {
    let registry = game.pack.assets.registry;
    let now = game.native_parts_seconds();
    for (pos, hose) in create_engine::hose::decode(data) {
        for i in 0..=create_engine::hose::MAX_DRAWN {
            game.native_part_clear(depot_key(pos, 220, i));
        }
        let Some((offset, per_tick)) = hose else {
            continue;
        };
        let model = |name: &str| state_with(registry, name, &[("facing", "up")]);
        let draw = |game: &mut Game<'_>, copy: u8, name: &str, down: f32, speed: f32| {
            let Some(state) = model(name) else {
                return;
            };
            let Some(h) = game.native_part_model(state) else {
                return;
            };
            game.native_part_glide(
                depot_key(pos, 220, copy),
                h,
                pos,
                1,
                0.0,
                0.0,
                [0.0, -down, 0.0, now],
                [0.0, -speed * 20.0, 0.0],
            );
        };
        for piece in create_engine::hose::pieces(offset) {
            let (copy, name, down, glides) = match piece {
                create_engine::hose::Piece::Magnet(d) => (0, "create:part_hose_magnet", d, true),
                create_engine::hose::Piece::HalfMagnet(d) => {
                    (0, "create:part_hose_half_magnet", d, true)
                }
                create_engine::hose::Piece::HalfRope(d) => (1, "create:part_hose_half", d, false),
                create_engine::hose::Piece::Rope(i, d) => (
                    2 + i.min(create_engine::hose::MAX_DRAWN - 2),
                    "create:part_hose_rope",
                    d,
                    false,
                ),
            };
            draw(game, copy, name, down, if glides { per_tick } else { 0.0 });
        }
    }
}

/// The stack of extensions on a whistle (`findRoot` from an extension): the first block below
/// that is no extension.
fn whistle_root(game: &Game<'_>, pos: [i32; 3]) -> Option<[i32; 3]> {
    let mut at = [pos[0], pos[1] - 1, pos[2]];
    for _ in 0..64 {
        let s = game.world.block(at[0], at[1], at[2]);
        match with_world(|w| w.table.get(s.0).map(|(b, _)| b)).flatten() {
            Some(Block::SteamWhistleExtension) => at[1] -= 1,
            Some(Block::SteamWhistle) => return Some(at),
            _ => return None,
        }
    }
    None
}

/// An extension's shape at `height` above `root`, if one is there.
fn extension_above(
    game: &Game<'_>,
    root: [i32; 3],
    height: i32,
) -> Option<create_engine::whistle::Shape> {
    let s = game.world.block(root[0], root[1] + height, root[2]);
    with_world(|w| match w.table.get(s.0) {
        Some((Block::SteamWhistleExtension, i)) => i.extender,
        _ => None,
    })
    .flatten()
}

/// `WhistleBlock.incrementSize`: the stack grows a semitone, with the xylophone note and the
/// block's hit sound at the new pitch.
fn grow_whistle(game: &mut Game<'_>, root: [i32; 3]) {
    use create_engine::whistle::Grow;
    let state = game.world.block(root[0], root[1], root[2]);
    let Some(size) = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.size)).flatten()
    else {
        return;
    };
    let grow = create_engine::whistle::grow(
        |h| extension_above(game, root, h),
        |h| {
            let s = game.world.block(root[0], root[1] + h, root[2]);
            game.res.behaviours().is_replaceable(s)
        },
    );
    let dimension = game.dim.id().to_owned();
    let (height, pitch) = match grow {
        Some(Grow::Double(h, p)) => {
            let at = [root[0], root[1] + h, root[2]];
            game.native_set_block_property_with(at, "shape", "double", ALL);
            (h, p)
        }
        Some(Grow::Place(h, p)) => {
            let registry = game.pack.assets.registry;
            if let Some(id) = state_with(
                registry,
                "create:steam_whistle_extension",
                &[("shape", "single"), ("size", size.name())],
            ) {
                game.native_set_block(&dimension, [root[0], root[1] + h, root[2]], id, ALL);
            }
            (h, p)
        }
        None => return,
    };
    let (x, y, z) = (root[0], root[1] + height, root[2]);
    // Copper's sounds: volume (1 + 1) / 2, the note at a quarter of it.
    game.native_run_command(&format!(
        "execute in {dimension} run playsound minecraft:block.note_block.xylophone blocks @a {x} {y} {z} 0.25 {pitch}"
    ));
    game.native_run_command(&format!(
        "execute in {dimension} run playsound minecraft:block.copper.hit blocks @a {x} {y} {z} 1 {pitch}"
    ));
}

/// `WrenchItem` on a whistle or extension: its size cycles (`getRotatedBlockState`), an
/// extension's whistle's; the extensions follow (`updateShape`). True when done.
pub(super) fn wrench_whistle(game: &mut Game<'_>, pos: [i32; 3], block: Block) -> bool {
    let root = match block {
        Block::SteamWhistle => Some(pos),
        Block::SteamWhistleExtension => whistle_root(game, pos),
        _ => return false,
    };
    let Some(root) = root else {
        return true;
    };
    let state = game.world.block(root[0], root[1], root[2]);
    let Some(size) = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.size)).flatten()
    else {
        return true;
    };
    game.native_set_block_property_with(root, "size", size.cycled().name(), ALL);
    true
}

/// A whistle placed (`WhistleBlock.getStateForPlacement`): on the clicked face of a tank
/// facing back into it, or on its top facing the way the player looks; powered as placed.
/// Without a tank that way it can't stay (Create refuses it): it is taken back.
pub(super) fn place_whistle(game: &mut Game<'_>, pos: [i32; 3], clicked: Dir) {
    let horizontal = create_engine::place::horizontal_direction(game.yaw);
    let (wall, facing) = create_engine::whistle::placement(clicked, horizontal);
    let tank = offset(pos, create_engine::whistle::attached(wall, facing));
    let s = game.world.block(tank[0], tank[1], tank[2]);
    let on_tank = with_world(|w| {
        w.table
            .get(s.0)
            .is_some_and(|(b, _)| matches!(b, Block::FluidTank | Block::CreativeFluidTank))
    }) == Some(true);
    let dimension = game.dim.id().to_owned();
    if !on_tank {
        if !game.creative {
            game.native_give_items("create:steam_whistle", 1);
        }
        if let Some(air) = game.pack.assets.registry.default_state("minecraft:air") {
            game.native_set_block(&dimension, pos, air, ALL);
        }
        return;
    }
    game.native_set_block_property(pos, "wall", flag(wall));
    game.native_set_block_property(pos, "facing", dir_name(facing));
}

fn tick_whistles(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::whistle;
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
        let s = game.world.block(pos[0], pos[1], pos[2]);
        let Some(Some(a)) = with_world(|w| match w.table.get(s.0) {
            Some((Block::SteamWhistle, i)) => Some(i.facing.map(|f| whistle::attached(i.wall, f))),
            _ => None,
        })
        .flatten() else {
            continue;
        };
        let t = offset(pos, a);
        let ts = game.world.block(t[0], t[1], t[2]);
        let held = with_world(|w| {
            w.table
                .get(ts.0)
                .is_some_and(|(b, _)| matches!(b, Block::FluidTank | Block::CreativeFluidTank))
        }) == Some(true);
        if !held {
            destroy(game, dimension, pos);
        }
    }
    let mut roots = Vec::new();
    for pos in extensions {
        let s = game.world.block(pos[0], pos[1], pos[2]);
        let Some(shape) = with_world(|w| match w.table.get(s.0) {
            Some((Block::SteamWhistleExtension, i)) => i.extender,
            _ => None,
        })
        .flatten() else {
            // Gone: the whistle below counts its pitch again.
            if let Some(root) = whistle_root(game, pos) {
                roots.push(root);
            }
            continue;
        };
        let below = game.world.block(pos[0], pos[1] - 1, pos[2]);
        let (below_whistle, below_ext, below_size) = with_world(|w| match w.table.get(below.0) {
            Some((Block::SteamWhistle, i)) => (true, None, i.size),
            Some((Block::SteamWhistleExtension, i)) => (false, i.extender, i.size),
            _ => (false, None, None),
        })
        .unwrap_or((false, None, None));
        if !whistle::extension_survives(below_whistle, below_ext) {
            destroy(game, dimension, pos);
            continue;
        }
        let above = extension_above(game, pos, 1).is_some();
        let want = whistle::connected(shape, above);
        if want != shape {
            game.native_set_block_property_with(pos, "shape", want.name(), ALL);
        }
        if let Some(size) = below_size {
            game.native_set_block_property_with(pos, "size", size.name(), ALL);
        }
        if let Some(root) = whistle_root(game, pos) {
            roots.push(root);
        }
    }
    with_world(|w| w.dims[dim].plumbing.whistle_checks.extend(roots));
    // `updatePitch`.
    for pos in with_world(|w| w.take_whistle_checks(dim)).unwrap_or_default() {
        let pitch = whistle::pitch(|h| extension_above(game, pos, h));
        with_world(|w| {
            let p = &mut w.dims[dim].plumbing;
            if let Some(wh) = p.whistles.get_mut(&pos)
                && wh.pitch != pitch
            {
                wh.pitch = pitch;
                // A sounding whistle changes its note at once.
                wh.replay_in = 0;
                p.sound_checks.push(pos);
            }
        });
    }
    // Whose sound may have started or stopped, and those sounding (`tickAudio`).
    let mut look = with_world(|w| w.take_sound_checks(dim)).unwrap_or_default();
    look.extend(with_world(|w| w.dims[dim].plumbing.whistling.clone()).unwrap_or_default());
    look.sort();
    look.dedup();
    for pos in look {
        let s = game.world.block(pos[0], pos[1], pos[2]);
        let Some((powered, size)) = with_world(|w| match w.table.get(s.0) {
            Some((Block::SteamWhistle, i)) => Some((i.powered, i.size.unwrap_or_default())),
            _ => None,
        })
        .flatten() else {
            continue;
        };
        let Some((sounds, mut wh)) = with_world(|w| {
            let sounds = whistle::sounds(powered, w.whistle_boiler(dim, pos));
            Some((sounds, *w.dims[dim].plumbing.whistles.get(&pos)?))
        })
        .flatten() else {
            continue;
        };
        let (x, y, z) = (
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]) + 0.5,
            f64::from(pos[2]) + 0.5,
        );
        if sounds {
            if !wh.sounding {
                // WHISTLE_CHIFF as it starts, `maxVolume * .175`.
                let p = whistle::chiff_pitch(size, wh.pitch);
                game.native_run_command(&format!(
                    "execute in {dimension} run playsound create:chiff record @a {x} {y} {z} 0.175 {p}"
                ));
            }
            if wh.replay_in == 0 {
                let p = whistle::sound_pitch(wh.pitch);
                game.native_run_command(&format!(
                    "execute in {dimension} run playsound {} record @a {x} {y} {z} 1 {p}",
                    size.sound()
                ));
                wh.replay_in = whistle::replay_ticks(wh.pitch);
            } else {
                wh.replay_in -= 1;
            }
            wh.sounding = true;
        } else if wh.sounding {
            // `fadeOut`: the loop stops (every whistle of that sound; the others start theirs
            // again at once).
            game.native_run_command(&format!(
                "execute in {dimension} run stopsound @a record {}",
                size.sound()
            ));
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
        with_world(|w| {
            let p = &mut w.dims[dim].plumbing;
            p.whistles.insert(pos, wh);
            p.whistling.retain(|q| *q != pos);
            if wh.sounding {
                p.whistling.push(pos);
            }
        });
    }
}

/// `destroyBlock(pos, true)`: the block breaks with its drops.
fn destroy(game: &mut Game<'_>, dimension: &str, pos: [i32; 3]) {
    game.native_run_command(&format!(
        "execute in {dimension} run setblock {} {} {} air destroy",
        pos[0], pos[1], pos[2]
    ));
}

/// Smart pipe filters (x, y, z, fluid), hoses (x, y, z, offset × 1000, fluid, amount) and
/// creative tanks (x, y, z, fluid; 255: none) saved with a column, under `create_fluids`.
const KEY: &str = "create_fluids";

pub(super) fn chunk_data_save(
    dimension: &str,
    chunk: [i32; 2],
    data: &mut horizon_world::nbt::Compound,
) {
    use horizon_world::nbt::{Compound, Tag};
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let here = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]);
    let index = |f: Option<Fluid>| i32::from(f.map_or(255, Fluid::index));
    let (filters, hoses, creative) = with_world(|w| {
        let p = &w.dims[dim].plumbing;
        let mut filters: Vec<_> = p
            .filters
            .iter()
            .filter(|(q, _)| here(q))
            .map(|(q, f)| (*q, *f))
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
        (filters, hoses, creative)
    })
    .unwrap_or_default();
    if filters.is_empty() && hoses.is_empty() && creative.is_empty() {
        data.remove(&KEY.into());
        return;
    }
    let mut own = Compound::new();
    let v: Vec<i32> = filters
        .iter()
        .flat_map(|(p, f)| [p[0], p[1], p[2], index(Some(*f))])
        .collect();
    own.insert("filters".into(), Tag::IntArray(v));
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
    data.insert(KEY.into(), Tag::Compound(own));
}

/// What a column saved, before its blocks are placed (they take it back when they are).
pub(super) fn chunk_load(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    use horizon_world::nbt::Tag;
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let Some(data) = Game::native_chunk_data(chunk, KEY) else {
        return;
    };
    let ints = |k: &str| match data.get(&k.into()) {
        Some(Tag::IntArray(v)) => v.clone(),
        _ => Vec::new(),
    };
    let fluid = |i: i32| Fluid::from_index(i.clamp(0, 255) as u8);
    let filters = ints("filters");
    let hoses = ints("hoses");
    let creative = ints("creative_tanks");
    with_world(|w| {
        let p = &mut w.dims[dim].plumbing;
        for c in filters.chunks_exact(4) {
            if let Some(f) = fluid(c[3]) {
                p.filters.insert([c[0], c[1], c[2]], f);
            }
        }
        for c in hoses.chunks_exact(6) {
            let tank = create_engine::tank::Tank {
                fluid: fluid(c[4]),
                amount: c[5].clamp(0, create_engine::hose::CAPACITY as i32) as u32,
            };
            p.hose_saved
                .insert([c[0], c[1], c[2]], ((c[3].max(0) as f32) / 1000.0, tank));
        }
        for c in creative.chunks_exact(4) {
            p.creative_saved.insert([c[0], c[1], c[2]], fluid(c[3]));
        }
    });
}

/// Client valve pointers: where each is, and those moving (the only ones that tick).
#[derive(Default)]
pub(super) struct ClientValves {
    pointers: std::collections::HashMap<[i32; 3], create_engine::fluids::Pointer>,
    moving: Vec<[i32; 3]>,
}

thread_local! {
    pub(super) static VALVES: std::cell::RefCell<ClientValves> = std::cell::RefCell::new(ClientValves::default());
}

/// A valve's speed on the client (`onSpeedChanged`): its pointer sets off, drawn where it is.
pub(super) fn valve_shown(game: &mut Game<'_>, pos: [i32; 3], info: &StateInfo, speed: f32) {
    let p = VALVES.with_borrow_mut(|v| {
        let p = v
            .pointers
            .entry(pos)
            .or_insert_with(|| create_engine::fluids::Pointer::at(info.enabled));
        p.speed_changed(speed);
        let p = *p;
        if p.moving() && !v.moving.contains(&pos) {
            v.moving.push(pos);
        }
        p
    });
    draw_pointer(game, pos, info, p.angle(), p.angle());
}

/// Each client tick: moving pointers take a step (`tickChaser`) and glide to it.
pub(super) fn tick_client_valves(game: &mut Game<'_>, table: &create_engine::world::StateTable) {
    let moving = VALVES.with_borrow_mut(|v| std::mem::take(&mut v.moving));
    if moving.is_empty() {
        return;
    }
    let mut still = Vec::new();
    for pos in moving {
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let Some(info) = table
            .get(state.0)
            .filter(|(b, _)| *b == Block::FluidValve)
            .map(|(_, i)| *i)
        else {
            VALVES.with_borrow_mut(|v| v.pointers.remove(&pos));
            continue;
        };
        let Some(mut p) = VALVES.with_borrow(|v| v.pointers.get(&pos).copied()) else {
            continue;
        };
        let from = p.angle();
        p.tick(info.enabled);
        // A pointer that arrived is drawn still there; one moving glides over the tick.
        let start = if p.moving() { from } else { p.angle() };
        draw_pointer(game, pos, &info, start, p.angle());
        VALVES.with_borrow_mut(|v| v.pointers.insert(pos, p));
        if p.moving() {
            still.push(pos);
        }
    }
    VALVES.with_borrow_mut(|v| v.moving.extend(still));
}

/// `FluidValveRenderer`'s pointer: turned to the valve's facing (`horizontalAngle`, 90° down
/// from up for a side, 180° for down) and its extra quarter turn, then gliding from `from` to
/// `to` degrees about the facing over a tick.
fn draw_pointer(game: &mut Game<'_>, pos: [i32; 3], info: &StateInfo, from: f32, to: f32) {
    let Some(facing) = info.facing else {
        return;
    };
    let pipe = create_engine::fluids::valve_pipe_axis(facing, info.along_first);
    let shaft = create_engine::parts::directional_axis(facing, info.along_first);
    let registry = game.pack.assets.registry;
    let Some(state) = state_with(
        registry,
        "create:part_fluid_valve_pointer",
        &[("facing", "up")],
    ) else {
        return;
    };
    let yaw = match facing {
        Dir::West => 90.0,
        Dir::North => 180.0,
        Dir::East => 270.0,
        _ => 0.0f32,
    };
    let pitch = match facing {
        Dir::Up => 0.0,
        Dir::Down => 180.0,
        _ => 90.0f32,
    };
    let extra = create_engine::fluids::pointer_offset(pipe, shaft);
    let c = glam::Vec3::splat(0.5);
    let pose = glam::Mat4::from_translation(c)
        * glam::Mat4::from_rotation_y(yaw.to_radians())
        * glam::Mat4::from_rotation_x(pitch.to_radians())
        * glam::Mat4::from_rotation_y(extra.to_radians())
        * glam::Mat4::from_translation(-c);
    let Some(model) = game.native_part_model_posed(state, pose) else {
        return;
    };
    // About the facing's axis: its positive end turns the same way, its negative end back.
    let sign = if facing.offset().iter().sum::<i32>() > 0 {
        1.0
    } else {
        -1.0
    };
    let now = game.native_parts_seconds();
    let (a, b) = (from * sign, to * sign);
    let rate = (b - a) * 20.0;
    let axis = axis_index(facing.axis());
    game.native_part_set(
        create_engine::client::key(pos, create_engine::client::VALVE_POINTER_SLOT),
        model,
        pos,
        axis,
        rate / 6.0,
        if rate == 0.0 { b } else { a - rate * now },
    );
}

/// `create_debug fluid_state <x y z>`: what a valve, smart pipe, hose pulley, creative tank or
/// whistle there holds, for tests.
pub(super) fn plumbing_command(game: &mut Game<'_>, command: &str) -> bool {
    let Some(rest) = command.strip_prefix("create_debug fluid_state ") else {
        return false;
    };
    let player = game.native_acting_player();
    let dim = player
        .as_ref()
        .and_then(|p| dimension_index(&p.dimension))
        .unwrap_or(0);
    let here = player.as_ref().map(|p| p.position);
    let parts: Vec<i32> = rest
        .split_whitespace()
        .enumerate()
        .filter_map(|(n, t)| match t.strip_prefix('~') {
            Some(off) => Some(here?.get(n % 3)?.floor() as i32 + off.parse::<i32>().unwrap_or(0)),
            None => t.parse().ok(),
        })
        .collect();
    let line = match parts.get(0..3) {
        Some(&[x, y, z]) => fluid_state(dim, [x, y, z]),
        _ => "usage: create_debug fluid_state <x> <y> <z>".into(),
    };
    game.native_feedback(horizon_client::text::Text::literal(line));
    true
}

fn fluid_state(dim: usize, pos: [i32; 3]) -> String {
    let [x, y, z] = pos;
    let name = |f: Option<Fluid>| f.map_or("none", Fluid::id);
    with_world(|w| {
        let p = &w.dims[dim].plumbing;
        if let Some(v) = p.valves.get(&pos) {
            return format!(
                "valve at {x} {y} {z}: pointer {:.2} toward {:.0}",
                v.value, v.target
            );
        }
        if let Some(h) = p.hoses.get(&pos) {
            return format!(
                "hose pulley at {x} {y} {z}: offset {:.2}, moving {}, tank {} {}, infinite {}",
                h.offset,
                h.moving,
                name(h.tank.fluid),
                h.tank.amount,
                h.drainer.infinite || h.filler.infinite
            );
        }
        if let Some(f) = p.creative.get(&pos) {
            return format!("creative fluid tank at {x} {y} {z}: {}", name(*f));
        }
        if let Some(wh) = p.whistles.get(&pos) {
            return format!(
                "whistle at {x} {y} {z}: pitch {}, sounding {}",
                wh.pitch, wh.sounding
            );
        }
        if let Some(f) = p.filters.get(&pos) {
            return format!("smart pipe at {x} {y} {z}: filter {}", f.id());
        }
        format!("nothing of the fluids' at {x} {y} {z}")
    })
    .unwrap_or_default()
}
