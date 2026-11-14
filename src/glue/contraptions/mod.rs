//! Create's contraptions: mechanical, windmill and clockwork bearings, gantries, mechanical
//! pistons, rope and elevator pulleys, chassis, and what rides them (`actors`). Their structures
//! leave the world while they move and come back where they stop, as Create's
//! `ControlledContraptionEntity` does; the rules are the engine's (`contraption`, `piston`,
//! `gantry`, `pulley`, `elevator`, `clock`, `lifts`, `actors`, `glue`).
//!
//! A moving structure is not an entity here: the mod API's entity types are template copies
//! (no mod entity carrying blocks), so each structure is kept by the engine, keyed by its
//! anchor (a bearing, piston, pulley, or the shaft a gantry carriage started on), saved with
//! the anchor's column, and drawn by the anchor's block entity (`draw`), which carries it to
//! clients in its data.
//!
//! What the API cannot do, left out (for HORIZON_API.md's gaps):
//! - Moving structures have no collision and carry nothing standing on them: there is no
//!   level collider an entity's motion reads (`notes/moving-structures-note.md`), and no mod
//!   entity type to be one. Create's `ContraptionCollider` is not run.
//! - A moving block cannot be clicked, and riders cannot sit on a moving structure's seats
//!   (`Contraption.seats`): there is no moving entity to pick or ride.
//! - Moving block entities other than chests and barrels are refused, as the legacy did:
//!   their block entity data has no general way to ride along. A bearing's structure carries
//!   no storage, glue or controls, as the legacy's didn't.
//! - The structure is lit with its anchor's light, not each block's own (one
//!   `BlockEntityDrawContext` light per draw).
//! - A clockwork bearing's top turns at its kinetic speed, not with the hour hand
//!   (`BearingRenderer`): the top is the kinetic block entity's part.

pub mod actors;
mod bearings;
mod blast;
pub mod controller;
pub mod draw;
mod lifts;
pub mod seats;

use super::server::ServerLevel;
use super::states;
use super::subsystem::Subsystem;
use super::world::with_world;
use create_engine::Block;
use create_engine::contraption::{Chassis, Found, Sticky};
use create_engine::kinetics::{Axis, Dir};
use mod_api::{
    BlockClick, BlockState, BlockUpdateFlags, Compound, Flow, Host, ItemStack, PlayerRef,
    RandomSource, RegisterCommandsEvent, Registrar, RegistrationContext, RunModCommandEvent,
    ServerHost,
};
use std::collections::HashMap;
use std::sync::RwLock;

pub use super::kinetics::dir_name;

pub struct Contraptions;

impl Subsystem for Contraptions {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        bearings::register(r);
        lifts::register(r);
        actors::register(r);
    }

    fn world_closed(&self) {
        bearings::forget();
        lifts::forget();
        actors::forget();
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        bearings::chunk_load(lv, data);
        lifts::chunk_load(lv, data);
        actors::glue_chunk_load(lv.dim, data);
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        bearings::chunk_save(lv, chunk, data);
        lifts::chunk_save(lv, chunk, data);
        actors::glue_chunk_save(lv.dim, chunk, data);
    }

    fn chunk_unload(&self, dim: usize, chunk: [i32; 2]) {
        actors::glue_chunk_unload(dim, chunk);
    }

    /// `MechanicalBearingBlockEntity.onSpeedChanged` (`assembleNextTick`) and
    /// `GantryShaftBlockEntity.onSpeedChanged` (the carriages on it try to start), at the next
    /// level tick.
    fn speed_changed(&self, lv: &mut ServerLevel<'_>, pos: [i32; 3], _speed: f32) {
        match lv.create_block(pos) {
            Some(Block::MechanicalBearing) => bearings::bearing_speed_changed(lv.dim, pos),
            Some(Block::GantryShaft) => bearings::gantry_carriages_on(lv, pos),
            _ => {}
        }
    }

    /// Before the kinetic solve, in the legacy order: mechanical bearings and their actors,
    /// gantries, windmills, stickers, pistons, mounted blocks, then clocks, pulleys and
    /// elevators.
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        bearings::tick_bearings(lv);
        bearings::tick_bearing_actors(lv);
        bearings::tick_gantries(lv);
        bearings::tick_windmills(lv);
        actors::tick_stickers(lv);
        bearings::tick_pistons(lv);
        actors::tick_mounted(lv);
        lifts::tick_lifts(lv);
    }

    fn register_commands(&self, _host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        actors::register_commands(event);
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        actors::run_command(host, event)
    }
}

// ---------------------------------------------------------------------------------------------
// Block states as the engine numbers them

/// The numbers of the states that are not Create's: the engine's structures hold a `u32` per
/// block, Create's states are `states::FIRST..`, and every other state a structure takes gets
/// the next number below that, for as long as the process runs (never saved: saves hold the
/// state's text).
#[derive(Default)]
struct Others {
    by_state: HashMap<BlockState, u32>,
    by_id: Vec<BlockState>,
}

static OTHERS: RwLock<Option<Others>> = RwLock::new(None);

/// The engine's number for `state`: Create's own id, or one given to any other state.
pub fn num(state: BlockState) -> u32 {
    if let Some(id) = states::id(state) {
        return id;
    }
    if let Some(id) = OTHERS
        .read()
        .ok()
        .and_then(|o| o.as_ref()?.by_state.get(&state).copied())
    {
        return id;
    }
    let mut o = OTHERS.write().unwrap_or_else(|e| e.into_inner());
    let o = o.get_or_insert_with(Others::default);
    if let Some(&id) = o.by_state.get(&state) {
        return id;
    }
    o.by_id.push(state);
    let id = o.by_id.len() as u32;
    o.by_state.insert(state, id);
    id
}

/// The state numbered `id`.
pub fn unnum(id: u32) -> Option<BlockState> {
    if id >= states::FIRST {
        return states::state(id);
    }
    let i = id.checked_sub(1)? as usize;
    OTHERS.read().ok()?.as_ref()?.by_id.get(i).copied()
}

/// `minecraft:oak_log[axis=y]`: the text of state `id`, as saves keep it (`state_text`).
pub fn state_text(host: &dyn Host, id: u32) -> Option<String> {
    host.block_state_string(unnum(id)?)
}

/// The state of a saved text, numbered (`state_of_text`).
pub fn state_of_text(host: &dyn Host, text: &str) -> Option<u32> {
    host.block_state_id(text).map(num)
}

/// The name and properties of state `id`.
pub fn name_props(host: &dyn Host, id: u32) -> Option<(String, Vec<(String, String)>)> {
    let text = state_text(host, id)?;
    let (name, props) = states::split(&text);
    Some((
        name.to_owned(),
        props
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
    ))
}

fn prop<'a>(props: &'a [(String, String)], key: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn flag(props: &[(String, String)], key: &str) -> bool {
    prop(props, key) == Some("true")
}

pub fn dir_of(name: &str) -> Option<Dir> {
    Dir::ALL.into_iter().find(|d| dir_name(*d) == name)
}

fn axis_of(name: Option<&str>) -> Axis {
    match name {
        Some("x") => Axis::X,
        Some("z") => Axis::Z,
        _ => Axis::Y,
    }
}

/// `name[k=v,..]` as a state, numbered (`state_with`).
pub fn state_with(host: &dyn Host, name: &str, props: &[(&str, &str)]) -> Option<u32> {
    let text = if props.is_empty() {
        name.to_owned()
    } else {
        let p: Vec<String> = props.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{name}[{}]", p.join(","))
    };
    host.block_state_id(&text).map(num)
}

/// The block and facts of a Create state.
pub fn create_info(id: u32) -> Option<(Block, create_engine::parts::StateInfo)> {
    create_engine::states::index(id)?;
    with_world(|w| w.table.get(id).map(|(b, i)| (b, *i))).flatten()
}

fn add(p: [i32; 3], o: [i32; 3]) -> [i32; 3] {
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

fn center(p: [i32; 3]) -> [f64; 3] {
    p.map(|v| f64::from(v) + 0.5)
}

const AIRS: [&str; 3] = ["minecraft:air", "minecraft:cave_air", "minecraft:void_air"];

impl ServerLevel<'_> {
    /// The state at `pos`, numbered; None where nothing is loaded.
    pub fn state_num(&self, pos: [i32; 3]) -> Option<u32> {
        self.block_state(pos).map(num)
    }

    /// The Create block at `pos`.
    pub fn create_block(&self, pos: [i32; 3]) -> Option<Block> {
        create_info(self.block(pos)).map(|(b, _)| b)
    }

    /// The name and properties of the block at `pos`.
    pub fn name_props(&self, pos: [i32; 3]) -> Option<(String, Vec<(String, String)>)> {
        name_props(&*self.host, self.state_num(pos)?)
    }

    /// The name of the block at `pos`.
    pub fn block_name(&self, pos: [i32; 3]) -> Option<String> {
        self.name_props(pos).map(|(n, _)| n)
    }

    /// Sets numbered state `id` at `pos`.
    pub fn set_num(&mut self, pos: [i32; 3], id: u32, flags: BlockUpdateFlags) -> bool {
        match unnum(id) {
            Some(s) => self.set_state(pos, s, flags),
            None => false,
        }
    }

    /// Air at `pos` (`removeBlocksFromWorld`).
    pub fn set_air(&mut self, pos: [i32; 3], flags: BlockUpdateFlags) -> bool {
        match self.host.block_state_id("minecraft:air") {
            Some(air) => self.set_state(pos, air, flags),
            None => false,
        }
    }

    /// Whether `pos` holds air (`minecraft:air` or `cave_air`), the cells a turned structure
    /// goes back into.
    fn is_free(&self, pos: [i32; 3]) -> bool {
        self.block_name(pos)
            .is_some_and(|n| n == "minecraft:air" || n == "minecraft:cave_air")
    }

    /// A cell a moving structure passes (air, or a fluid; `canBeReplaced` with no collision).
    fn passable(&self, pos: [i32; 3]) -> bool {
        self.block_name(pos).is_some_and(|n| {
            AIRS.contains(&n.as_str()) || n == "minecraft:water" || n == "minecraft:lava"
        })
    }

    /// The block of state `id` as an item, dropped at the middle of `pos` (a block that could
    /// not go back).
    fn drop_block(&mut self, pos: [i32; 3], id: u32) {
        let Some(state) = unnum(id) else {
            return;
        };
        let Some(item) = self.host.block_item(state.block()) else {
            return;
        };
        self.drop_stack(center(pos), [0.0; 3], ItemStack::of(item), 10);
    }

    /// An item entity at `at` with `motion` (`new ItemEntity(..)`, `setDeltaMovement`), picked
    /// up after `delay` ticks.
    pub fn drop_stack(&mut self, at: [f64; 3], motion: [f64; 3], stack: ItemStack, delay: u32) {
        let _ = self.level.spawn_item_with_motion(
            &mut *self.host,
            at[0],
            at[1],
            at[2],
            stack,
            motion,
            delay,
        );
    }

    /// `world.random.nextFloat()`.
    pub fn random_float(&mut self) -> f32 {
        self.level
            .with_random(&mut *self.host, |r| r.next_float())
            .unwrap_or(0.5)
    }
}

/// Puts `blocks` (world positions) back: where a cell is free, else dropped
/// (`Contraption.addBlocksToWorld`); `free` says which cells take a block.
fn put_back_blocks(
    lv: &mut ServerLevel<'_>,
    blocks: &[([i32; 3], u32)],
    free: impl Fn(&ServerLevel<'_>, [i32; 3]) -> bool,
) {
    for &(p, state) in blocks {
        if free(lv, p) {
            lv.set_num(p, state, super::server::update_all());
        } else {
            lv.drop_block(p, state);
        }
    }
}

/// Takes `blocks` (offsets from `base`) out of the world (`removeBlocksFromWorld`).
fn take_blocks(lv: &mut ServerLevel<'_>, base: [i32; 3], blocks: &[([i32; 3], u32)]) {
    for (o, _) in blocks {
        lv.set_air(add(base, *o), super::server::UPDATE_CLIENTS);
    }
}

/// A radial chassis as the search reads it: its axis, glued sides and range.
fn radial_chassis(dim: usize, p: [i32; 3], props: &[(String, String)]) -> Chassis {
    let axis = axis_of(prop(props, "axis"));
    let sides = Dir::ALL.into_iter().fold(0u8, |acc, d| {
        let glued =
            create_engine::contraption::radial_side(axis, d).is_some_and(|k| flag(props, k));
        if glued { acc | d.bit() } else { acc }
    });
    Chassis {
        secondary: false,
        axis,
        sticky_top: false,
        sticky_bottom: false,
        range: chassis_range(dim, p),
        radial: Some(sides),
    }
}

fn chassis_range(dim: usize, p: [i32; 3]) -> i32 {
    with_world(|w| w.dims[dim].chassis_range.get(&p).copied())
        .flatten()
        .unwrap_or(create_engine::contraption::CHASSIS_RANGE)
}

/// What a structure search finds at `p` (`contraption_found`): None for air and fluids. Blocks
/// Create can't move (unbreakable ones) and blocks whose block entity would be lost (moving
/// block entities isn't ported: refused rather than emptied; Create's own state-keeping blocks
/// and chests and barrels ride) are unmovable.
pub fn found(lv: &ServerLevel<'_>, p: [i32; 3]) -> Option<Found> {
    let id = lv.state_num(p)?;
    let (name, props) = name_props(&*lv.host, id)?;
    if AIRS.contains(&name.as_str()) || name == "minecraft:water" || name == "minecraft:lava" {
        return None;
    }
    let sail =
        name == "create:sail_frame" || (name.starts_with("create:") && name.ends_with("_sail"));
    let sail_facing = sail
        .then(|| prop(&props, "facing").and_then(dir_of))
        .flatten();
    let foreign_entity = lv
        .host
        .block_entity(lv.level, p)
        .is_some_and(|e| e.kind(&*lv.host) != super::kinetics::kind())
        && !actors::is_storage(lv, p);
    let unmovable = matches!(
        name.as_str(),
        "minecraft:bedrock"
            | "minecraft:barrier"
            | "minecraft:end_portal_frame"
            | "minecraft:command_block"
            | "minecraft:chain_command_block"
            | "minecraft:repeating_command_block"
            | "minecraft:structure_block"
            | "minecraft:jigsaw"
    ) || foreign_entity;
    let sticker = (name == "create:sticker" && flag(&props, "extended"))
        .then(|| prop(&props, "facing").and_then(dir_of))
        .flatten();
    let chassis = match name.as_str() {
        // LinearChassisBlock: its kind, axis and glued ends.
        "create:linear_chassis" | "create:secondary_linear_chassis" => Some(Chassis {
            secondary: name == "create:secondary_linear_chassis",
            axis: axis_of(prop(&props, "axis")),
            sticky_top: flag(&props, "sticky_top"),
            sticky_bottom: flag(&props, "sticky_bottom"),
            range: chassis_range(lv.dim, p),
            radial: None,
        }),
        "create:radial_chassis" => Some(radial_chassis(lv.dim, p, &props)),
        _ => None,
    };
    Some(Found {
        state: id,
        sail_facing,
        sticky: match name.as_str() {
            "minecraft:slime_block" => Some(Sticky::Slime),
            "minecraft:honey_block" => Some(Sticky::Honey),
            _ => None,
        },
        glued: actors::glued_at(lv.dim, p),
        sticker,
        movable: !unmovable,
        chassis,
    })
}

/// A block state turned `quarter`° about `axis` (`StructureTransform`): its `facing` or `axis`
/// turned with it (`rotateFacing`: `getClockWise(axis)`, `rotation.ordinal()` times); a state
/// that has no such turned form stays as it was.
fn turned_state(host: &dyn Host, state: u32, quarter: i32, axis: Axis) -> u32 {
    let turns = create_engine::contraption::clockwise_turns(quarter);
    if turns == 0 {
        return state;
    }
    let Some((name, mut props)) = name_props(host, state) else {
        return state;
    };
    let turn_dir = |d: Dir| (0..turns).fold(d, |d, _| create_engine::wrench::clockwise(d, axis));
    if let Some(d) = prop(&props, "facing").and_then(dir_of) {
        set_prop(&mut props, "facing", dir_name(turn_dir(d)));
    } else if let Some(a) = prop(&props, "axis") {
        let d = match a {
            "x" => Dir::East,
            "y" => Dir::Up,
            _ => Dir::South,
        };
        let name = match turn_dir(d).axis() {
            Axis::X => "x",
            Axis::Y => "y",
            Axis::Z => "z",
        };
        set_prop(&mut props, "axis", name);
    } else {
        return state;
    }
    let p: Vec<(&str, &str)> = props
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    state_with(host, &name, &p).unwrap_or(state)
}

fn set_prop(props: &mut [(String, String)], key: &str, value: &str) {
    if let Some(e) = props.iter_mut().find(|(k, _)| k == key) {
        e.1 = value.to_owned();
    }
}

/// The player clicking, sneaking (`isShiftKeyDown`), and what their hand holds.
struct Clicker {
    player: Option<PlayerRef>,
    sneaking: bool,
    held: Option<String>,
}

fn clicker(host: &dyn ServerHost, click: &BlockClick<'_>) -> Clicker {
    let player = host.player(click.player);
    let sneaking = player
        .and_then(|p| p.entity().is_shift_key_down(host))
        .unwrap_or(false);
    let held = click.stack.map(|s| s.item().id(host));
    Clicker {
        player,
        sneaking,
        held,
    }
}

/// Runs `f` in the level a click was in.
fn in_click_level<R>(
    host: &mut dyn ServerHost,
    click: &BlockClick<'_>,
    f: impl FnOnce(&mut ServerLevel<'_>) -> R,
) -> Option<R> {
    let dimension = click.dimension.key().id().to_owned();
    let mut lv = ServerLevel::new(host, &dimension)?;
    Some(f(&mut lv))
}
