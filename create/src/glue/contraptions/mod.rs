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
mod superglue;

use super::server::ServerLevel;
use super::states;
use super::subsystem::Subsystem;
use super::world::with_world;
use create_engine::Block;
use create_engine::contraption::{Chassis, Found, Sticky};
use create_engine::kinetics::{Axis, Dir};
use mod_api::{
    BlockClick, BlockState, BlockUpdateFlags, Compound, Flow, Host, ItemStack, LeftClickBlockEvent,
    PlayerRef, RandomSource, RegisterCommandsEvent, Registrar, RegistrationContext,
    RunModCommandEvent, ServerHost,
};
use std::collections::HashMap;
use std::sync::RwLock;

pub use super::kinetics::dir_name;
// The chassis group a value box scrolls the range of (legacy `contraptions::chassis_group`).
pub use bearings::chassis_group;

pub struct Contraptions;

impl Subsystem for Contraptions {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        bearings::register(r);
        lifts::register(r);
        actors::register(r);
        superglue::register(r);
    }

    fn world_closed(&self) {
        bearings::forget();
        lifts::forget();
        actors::forget();
        superglue::forget();
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        bearings::chunk_load(lv, data);
        lifts::chunk_load(lv, data);
        actors::glue_chunk_load(&*lv.host, lv.dim, data);
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        bearings::chunk_save(lv, chunk, data);
        lifts::chunk_save(lv, chunk, data);
        actors::glue_chunk_save(&*lv.host, lv.dim, chunk, data);
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

    /// A left click with super glue in hand takes the glue off the block hit.
    fn left_click_block(&self, host: &mut dyn ServerHost, event: &mut LeftClickBlockEvent) -> Flow {
        if superglue::left_click(host, event) {
            return Flow::Cancel;
        }
        Flow::Continue
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

/// Whether the block of state `id` is brittle (`BlockMovementChecks.isBrittle`): it hangs on to
/// another and breaks without it.
fn brittle_state(host: &dyn Host, id: u32) -> bool {
    name_props(host, id).is_some_and(|(n, p)| brittle(&n, &p))
}

/// Blocks that can't be broken (`getDestroySpeed == -1`) and so are never replaced.
fn unbreakable(name: &str) -> bool {
    matches!(
        name,
        "minecraft:bedrock"
            | "minecraft:barrier"
            | "minecraft:end_portal_frame"
            | "minecraft:end_portal"
            | "minecraft:end_gateway"
            | "minecraft:nether_portal"
            | "minecraft:command_block"
            | "minecraft:chain_command_block"
            | "minecraft:repeating_command_block"
            | "minecraft:structure_block"
            | "minecraft:jigsaw"
            | "minecraft:light"
    )
}

/// Puts `blocks` (world positions) back (`Contraption.addBlocksToWorld`): the sturdy ones first,
/// then the brittle ones that hang on them. A cell `free` says takes a block takes it; a block
/// that can't be broken there, or a brittle block meeting something else, drops the moving
/// block; anything else there is broken (dropping) for it.
fn put_back_blocks(
    lv: &mut ServerLevel<'_>,
    blocks: &[([i32; 3], u32)],
    free: impl Fn(&ServerLevel<'_>, [i32; 3]) -> bool,
) {
    let flags = super::server::update_all() | BlockUpdateFlags::MOVE_BY_PISTON;
    for brittles in [false, true] {
        for &(p, state) in blocks {
            if brittle_state(&*lv.host, state) != brittles {
                continue;
            }
            if !free(lv, p) {
                let here = lv.block_name(p).unwrap_or_default();
                if brittles || unbreakable(&here) {
                    lv.drop_block(p, state);
                    continue;
                }
                lv.destroy_block(p, true);
            }
            lv.set_num(p, state, flags);
        }
    }
}

/// Takes `blocks` (offsets from `base`) out of the world (`removeBlocksFromWorld`).
fn take_blocks(lv: &mut ServerLevel<'_>, base: [i32; 3], blocks: &[([i32; 3], u32)]) {
    let cells: Vec<([i32; 3], u32)> = blocks.iter().map(|(o, s)| (add(base, *o), *s)).collect();
    remove_cells(lv, &cells);
}

/// `removeBlocksFromWorld` for blocks at world positions: brittle ones first, each left as air
/// with no drops and no neighbour updates (nothing hanging on them breaks off), then their
/// neighbours hear of it.
fn remove_cells(lv: &mut ServerLevel<'_>, cells: &[([i32; 3], u32)]) {
    let flags = BlockUpdateFlags::MOVE_BY_PISTON
        | BlockUpdateFlags::SUPPRESS_DROPS
        | BlockUpdateFlags::KNOWN_SHAPE
        | BlockUpdateFlags::CLIENTS;
    for brittles in [true, false] {
        for &(p, state) in cells {
            if brittle_state(&*lv.host, state) == brittles {
                lv.set_air(p, flags);
            }
        }
    }
    for &(p, _) in cells {
        lv.update_signal_neighbors(p);
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
        attached: attached_sides(&name, &props),
        unsupportive: unsupportive_faces(&name, &props),
    })
}

fn bits(dirs: &[Dir]) -> u8 {
    dirs.iter().fold(0, |acc, d| acc | d.bit())
}

/// `BlockMovementChecksImpl.isBrittleFallback`: blocks that hang on to another and break
/// without it.
fn brittle(name: &str, props: &[(String, String)]) -> bool {
    let id = name.strip_prefix("minecraft:").unwrap_or(name);
    prop(props, "hanging").is_some()
        || id == "ladder"
        || torch(name)
        || id.ends_with("_sign")
        || id.ends_with("_pressure_plate")
        || (prop(props, "face").is_some()
            && !matches!(name, "minecraft:grindstone" | "create:packager_link"))
        || (rail(name) && name != "create:cart_assembler")
        || diode(name)
        || id == "redstone_wire"
        || wool_carpet(name)
        || matches!(
            name,
            "create:steam_whistle" | "create:steam_whistle_extension" | "create:brass_funnel"
        )
}

fn torch(name: &str) -> bool {
    name.starts_with("minecraft:") && name.ends_with("torch")
}

fn rail(name: &str) -> bool {
    matches!(
        name,
        "minecraft:rail"
            | "minecraft:powered_rail"
            | "minecraft:detector_rail"
            | "minecraft:activator_rail"
            | "create:controller_rail"
    )
}

/// `DiodeBlock`: repeaters, comparators and Create's brass diodes and latches.
fn diode(name: &str) -> bool {
    matches!(
        name,
        "minecraft:repeater"
            | "minecraft:comparator"
            | "create:pulse_repeater"
            | "create:pulse_extender"
            | "create:pulse_timer"
            | "create:powered_latch"
            | "create:powered_toggle_latch"
    )
}

/// `WoolCarpetBlock`: the sixteen wool carpets (not moss).
fn wool_carpet(name: &str) -> bool {
    name.starts_with("minecraft:") && name.ends_with("_carpet") && !name.ends_with("moss_carpet")
}

/// `AttachedActorBlock`: harvesters, ploughs and rollers, hanging on to the block behind them.
fn attached_actor(name: &str) -> bool {
    matches!(
        name,
        "create:mechanical_harvester" | "create:mechanical_plough" | "create:mechanical_roller"
    )
}

/// `BlockMovementChecksImpl.isBlockAttachedTowardsFallback` (all but sails, fluid tanks and
/// vaults, stickers and bogeys, which the search reads on their own): the sides a block hangs on
/// to its neighbour by, a `Dir::bit` each.
pub(crate) fn attached_sides(name: &str, props: &[(String, String)]) -> u8 {
    use Dir::{Down, Up};
    let facing = prop(props, "facing").and_then(dir_of);
    let back = facing.map_or(0, |f| f.opposite().bit());
    let id = name.strip_prefix("minecraft:").unwrap_or(name);
    let wall_facing = |n: &str| n.ends_with("wall_torch") || n.ends_with("_wall_sign");
    if id == "ladder" || wall_facing(id) {
        return back;
    }
    if id.ends_with("_sign") && !id.ends_with("hanging_sign") {
        return Down.bit();
    }
    if id.ends_with("_door") && prop(props, "half").is_some() {
        return if prop(props, "half") == Some("lower") {
            bits(&[Up, Down])
        } else {
            Down.bit()
        };
    }
    if id.ends_with("_bed") {
        let toward = if prop(props, "part") == Some("head") {
            facing.map(Dir::opposite)
        } else {
            facing
        };
        return toward.map_or(0, Dir::bit);
    }
    if name == "create:redstone_link" || attached_actor(name) {
        return back;
    }
    if matches!(name, "create:hand_crank" | "create:nozzle") {
        return back;
    }
    if id.ends_with("_pressure_plate")
        || id == "flower_pot"
        || id.starts_with("potted_")
        || diode(name)
        || id == "redstone_wire"
        || wool_carpet(name)
        || torch(name)
        || (rail(name) && name != "create:cart_assembler")
        || name == "create:steam_whistle_extension"
    {
        return Down.bit();
    }
    if let Some(face) = prop(props, "face") {
        return match face {
            "ceiling" => Up.bit(),
            "floor" => Down.bit(),
            _ => back,
        };
    }
    if let Some(h) = prop(props, "hanging") {
        return if h == "true" { Up.bit() } else { Down.bit() };
    }
    if id == "bell" {
        return match prop(props, "attachment") {
            Some("floor") => Down.bit(),
            Some("ceiling") => Up.bit(),
            _ => facing.map_or(0, Dir::bit),
        };
    }
    if name == "create:steam_whistle" {
        return if flag(props, "wall") {
            facing.map_or(0, Dir::bit)
        } else {
            Down.bit()
        };
    }
    0
}

/// `BlockMovementChecksImpl.isNotSupportiveFallback` (but sails, read by the search): the faces
/// of a block that push nothing in front of them, a `Dir::bit` each.
pub(crate) fn unsupportive_faces(name: &str, props: &[(String, String)]) -> u8 {
    let facing = prop(props, "facing").and_then(dir_of);
    let front = facing.map_or(0, Dir::bit);
    let across = |f: Dir| bits(&Dir::ALL) & !bits(&[f, f.opposite()]);
    match name {
        "create:mechanical_drill"
        | "create:mechanical_bearing"
        | "create:mechanical_saw"
        | "create:portable_storage_interface" => front,
        "create:cart_assembler" | "create:rope_pulley" => Dir::Down.bit(),
        "create:mechanical_harvester" | "create:mechanical_plough" => front,
        "create:piston_extension_pole" | "create:mechanical_piston_head" => {
            facing.map_or(0, across)
        }
        "create:sticker" if !flag(props, "extended") => front,
        _ if wool_carpet(name) => Dir::Up.bit(),
        _ if name.ends_with("sliding_door") => 0,
        _ if brittle(name, props) => bits(&Dir::ALL),
        _ => 0,
    }
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
    /// The off hand's turn after an empty main hand. Create's blocks answer an empty main hand
    /// `SUCCESS` on the client too, so the off hand is never tried after it; here the client
    /// can't know, tries the off hand, and an empty-hand action must not run twice.
    after_empty_main: bool,
}

fn clicker(host: &dyn ServerHost, click: &BlockClick<'_>) -> Clicker {
    let player = host.player(click.player);
    let sneaking = player
        .and_then(|p| p.entity().is_shift_key_down(host))
        .unwrap_or(false);
    let held = click.stack.map(|s| s.item().id(host));
    let after_empty_main = click.hand == mod_api::Hand::Off
        && player.is_none_or(|p| {
            super::items::held(host, p, mod_api::Hand::Main).is_none_or(|s| s.count() == 0)
        });
    Clicker {
        player,
        sneaking,
        held,
        after_empty_main,
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
