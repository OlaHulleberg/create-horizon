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
//! The movement mode on a bearing's, piston's or pulley's value box (`RotationMode`,
//! `MovementMode`), a windmill's direction, a clock's hand arrangement and a roller's mode are
//! kept per block (`scroll_options`, saved as `modes`). A stopped piston or pulley its mode keeps
//! together waits for its speed to change (`waitingForSpeedChange`); pistons and gantries stop
//! against the world as `ContraptionCollider.collideBlocks` has them. Chests, barrels, item
//! vaults and toolboxes ride every contraption as mounted storage (saved with it), bearings,
//! windmills and clocks included.
//!
//! What the API cannot do, left out (for API_GAPS.md's gaps):
//! - Entities can't stand on, be pushed by or ride moving structures: there is no level
//!   collider an entity's motion reads (`API_GAPS.md`, moving colliders), and no mod entity
//!   type to be one.
//! - A moving block cannot be clicked, and riders cannot sit on a moving structure's seats
//!   (`Contraption.seats`): there is no moving entity to pick or ride. Contraption controls on a
//!   structure an anchor draws (piston, gantry, pulley, bearing, windmill, elevator) are clicked
//!   and scrolled through the client's own raycast of what it draws, turned back into the
//!   structure's frame (`draw::use_key`, `draw::scroll`); on a minecart's contraption (drawn by
//!   display entities) with `/create_debug controls`.
//! - Each moving block is lit with the world's light where the structure was last sent from
//!   (the server reads it: `LevelRef::brightness` is the server's only), not where it is each
//!   frame.
//! - A clockwork bearing's top turns at its kinetic speed, not with the hour hand
//!   (`BearingRenderer`): the top is the kinetic block entity's part.
//!
//! Block entities ride with what they hold (`actors::carry_storage`, Create's
//! `StructureBlockInfo.nbt`), on bearings, pistons, gantries, pulleys, elevators, carts and
//! trains: Create's item slots (a depot's stack), its block entity's own data (a copycat's
//! material, a sequenced gearshift's program, a valve handle's angle), a deployer's hand and
//! mode, a tank's fluid (tanks keep their shape), and other block entities' saved data (a
//! furnace's items); and what the engine keeps for the block, which each subsystem takes from
//! its tables and puts back where the block goes as a saved one loads (`Subsystem::carry_out`,
//! `carry_in`): a creative motor's or speed controller's speed, a blaze burner's fuel, a basin's
//! fluids, filter slots and their amounts (smart chutes, brass funnels, basins, saws,
//! deployers), brass tunnels' side filters and mode, a redstone link's frequency, brass diodes,
//! analog levers, smart observers and threshold switches, a nixie tube's text, a display link's
//! target (turned with the structure) and source, spouts', drains' and creative tanks' fluids,
//! smart pipes' filters, a saw's items, crafters' covers and joined inputs, an ejector's target,
//! an arm's points (turned), a backtank's air, brackets, placards, placed clipboards, crates'
//! supplies, hatches' filters, copycats' materials (turned), a contraption block's movement mode
//! and a chassis' range. Saved with the moving structure (`actors::storage_to_nbt`).
//!
//! What their block entities hold is drawn while they move (`renderBlockEntities` with the
//! contraption's update tags: `draw::draw_carried`, `Subsystem::draw_carried`): a depot's or
//! ejector's stacks, a basin's items and fluids, a tank's, spout's or drain's fluid, a crafter's
//! item, a nixie tube's text, a display board's lines, a belt (its segments, casing and items:
//! a belt moves whole, `moveBelt`), filter slots' items, a chute's stack, an arm at rest,
//! copycats' materials, placards' items, brackets, vanilla chests, ender chests and shulker boxes
//! (their item's special model), signs' text and banners (`vanilla_draw`; a bed and a sign's
//! board are their block models). Blocks whose movement draws them (`disableBlockEntityRendering`:
//! deployers, whose held item Create does not draw riding either, drills, saws, harvesters,
//! rollers, interfaces, burners) are left to it. A part a riding block entity turns stands at its
//! phase. A crafter's grid mid-craft rides with it and goes on where it goes back.
//!
//! A sequenced gearshift's or valve handle's limit on a bearing, piston, pulley or gantry
//! (`sequencedAngleLimit`, `sequencedOffsetLimit`) is saved with the structure, a turning
//! bearing's turn as it follows from its last change; a moving gantry is saved with the column
//! its carriage started from. A structure loaded moving waits for its network's first solve
//! rather than stopping at the speed 0 it loads with.
//!
//! Deployers work while moving (`DeployerMovementBehaviour`), redstone contacts power the
//! contacts they pass (`ContactMovementBehaviour`), elevators open their sliding doors at floors
//! on the sides the floor's contact's door control lets open (`SlidingDoorMovementBehaviour`,
//! `DoorControlBehaviour`), and portable fluid interfaces reach a contraption's tanks.
//!
//! Actors' moving parts are drawn as Create's actor visuals draw them (`actor_draw`): drill heads
//! turn, harvester blades and roller wheels roll, saw blades run, deployers stroke and punch,
//! interfaces reach out when they dock, from the structure's motion and what the server tells of
//! its stall, its actors turned off and its interfaces. A carried mechanical bearing's top stays
//! at rest on the structure, as Create's does when nothing is stabilized on it
//! (`StabilizedBearingMovementBehaviour`).
//!
//! Every structure takes the glue that holds it out of the world (`Contraption.superglue`) and
//! puts it back where its blocks land, turned with them, and carries its contraption controls'
//! settings: controls set off turn their kind of actor off as it starts (`disableActorOnStart`),
//! a click turns kinds off and on (`ContraptionControlsMovingInteraction`), and each block put
//! back is off as the contraption has its kind (`stopMoving`); all of it saved with the
//! structure (`actors::extras_to_nbt`). An elevator's controls instead show its column's floors
//! (`ElevatorFloorListPacket`): scrolling on their button picks one, a click calls it, and the
//! indicator turns toward the floor called (`ContraptionControlsMovement` elevator mode).

pub mod actor_draw;
pub mod actors;
mod bearings;
pub(crate) use bearings::LAST_EXCEPTION;
use bearings::set_exception;

/// The key of a moving structure in its anchor's block entity data.
pub(crate) fn draw_key() -> &'static str {
    draw::KEY
}
mod blast;
pub mod controller;
pub mod draw;
mod lifts;
pub mod seats;
mod superglue;
mod vanilla_draw;

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
// The chassis group a value box scrolls the range of.
pub use bearings::chassis_group;
pub(crate) use lifts::{contact_floor, contact_wrenched};

pub struct Contraptions;

/// What a moving structure carries for a contraption block on it (`Subsystem::carry_out`).
const CARRIED_OPTION: &str = "ScrollOption";
const CARRIED_RANGE: &str = "ChassisRange";

impl Subsystem for Contraptions {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        bearings::register(r);
        lifts::register(r);
        actors::register(r);
        superglue::register(r);
    }

    fn world_closed(&self) {
        LOADED_LIMITS.with_borrow_mut(std::collections::HashSet::clear);
        bearings::forget();
        lifts::forget();
        actors::forget();
        superglue::forget();
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        bearings::chunk_load(lv, data);
        lifts::chunk_load(lv, data);
        lifts::contacts_load(lv, data);
        actors::glue_chunk_load(&*lv.host, lv.dim, data);
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        bearings::chunk_save(lv, chunk, data);
        lifts::chunk_save(lv, chunk, data);
        lifts::contacts_save(lv, chunk, data);
        actors::glue_chunk_save(&*lv.host, lv.dim, chunk, data);
    }

    fn chunk_unload(&self, dim: usize, chunk: [i32; 2]) {
        actors::glue_chunk_unload(dim, chunk);
    }

    /// The option on a contraption block's value box (`ScrollOptionBehaviour`: a bearing's or
    /// piston's movement mode, a windmill's direction, a clock's hands) and a chassis' range
    /// (`ChassisScrollValueBehaviour`) ride with it.
    fn carry_out(&self, lv: &mut ServerLevel<'_>, pos: [i32; 3], _b: Block, out: &mut Compound) {
        let dim = lv.dim;
        let (option, range) = with_world(|w| {
            let d = &mut w.dims[dim];
            (d.scroll_options.remove(&pos), d.chassis_range.remove(&pos))
        })
        .unwrap_or_default();
        if let Some(o) = option {
            out.insert(CARRIED_OPTION.into(), mod_api::Tag::Int(i32::from(o)));
        }
        if let Some(r) = range {
            out.insert(CARRIED_RANGE.into(), mod_api::Tag::Int(r));
        }
    }

    fn carry_in(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        _b: Block,
        data: &Compound,
        _turn: create_engine::contraption::Transform,
    ) {
        let dim = lv.dim;
        let int = |k: &str| match data.get(&k.into()) {
            Some(mod_api::Tag::Int(v)) => Some(*v),
            _ => None,
        };
        let (option, range) = (int(CARRIED_OPTION), int(CARRIED_RANGE));
        with_world(|w| {
            let d = &mut w.dims[dim];
            if let Some(o) = option {
                d.scroll_options.insert(pos, o.clamp(0, 255) as u8);
            }
            if let Some(r) = range {
                d.chassis_range.insert(pos, r);
            }
        });
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

    /// Before the kinetic solve, in this order: mechanical bearings and their actors,
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
        superglue::tick(lv);
    }

    /// A click on contraption controls riding a moving structure (`CONTROLS_CHANNEL`).
    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        if channel != draw::CONTROLS_CHANNEL {
            return false;
        }
        let Some((anchor, local, target)) = draw::decode_click(data) else {
            return true;
        };
        let Some(dimension) = player.level(&*host).and_then(|l| l.dimension().id(&*host)) else {
            return true;
        };
        if let Some(mut lv) = ServerLevel::new(host, &dimension) {
            actors::click_moving_controls(&mut lv, player, anchor, local, target);
        }
        true
    }

    /// The use key on contraption controls riding a moving structure (`ContraptionHandlerClient`
    /// raycasts the structures the client draws).
    fn interaction_key(
        &self,
        host: &mut dyn mod_api::ClientHost,
        event: &mut mod_api::InteractionKeyMappingTriggeredEvent,
    ) -> Flow {
        draw::use_key(host, event)
    }

    /// `ElevatorControlsHandler.onScroll`: scrolling on an elevator's contraption controls picks
    /// a floor.
    fn mouse_scrolling(
        &self,
        host: &mut dyn mod_api::ClientHost,
        event: &mut mod_api::MouseScrollingEvent,
    ) -> Flow {
        if draw::scroll(host, event.scroll[1]) {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    fn server_message(
        &self,
        _host: &mut dyn mod_api::ClientHost,
        channel: &str,
        data: &[u8],
    ) -> bool {
        if channel == actors::THROW_CHANNEL {
            actors::thrown(_host, data);
            return true;
        }
        if channel != superglue::CHANNEL {
            return false;
        }
        superglue::from_server(data);
        true
    }

    fn client_reset(&self) {
        superglue::client_reset();
    }

    fn render_level_stage(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::RenderLevelStageEvent<'_>,
    ) {
        superglue::render(host, event);
    }

    /// A right click with super glue in hand picks a corner, not the block hit.
    fn right_click_block(
        &self,
        mut host: mod_api::HostSide,
        event: &mut mod_api::RightClickBlockEvent<'_>,
    ) -> Flow {
        superglue::right_click_block(&mut host, event)
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
// Value boxes

/// The option a contraption block's value box is on (`ScrollOptionBehaviour.get`): a bearing's,
/// piston's or pulley's movement mode, a windmill's direction, a clock's hands, a roller's mode.
pub fn scroll_option(lv: &ServerLevel<'_>, pos: [i32; 3], block: Block) -> i32 {
    let dim = lv.dim;
    if block == Block::MechanicalRoller {
        return with_world(|w| w.dims[dim].rollers.get(&pos).map(|r| r.1 as i32))
            .flatten()
            .unwrap_or(0);
    }
    option_at(dim, pos)
}

/// The option kept for the block at `pos` (0, the first, when none is).
/// The key a saved structure keeps what is left of its sequenced turn's limit under
/// (`SequencedAngleLimit`, `SequencedOffsetLimit`), when it has one.
const SEQUENCED_LIMIT: &str = "SequencedLimit";

thread_local! {
    /// Structures loaded with a sequenced limit: the first speed check after the load keeps it
    /// (Create's block entity reads its speed with it, so no `onSpeedChanged` starts it again).
    static LOADED_LIMITS: std::cell::RefCell<std::collections::HashSet<(usize, [i32; 3])>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// What is left of a sequenced limit, into a saved structure's compound.
fn limit_to_nbt(c: &mut Compound, limit: Option<f32>) {
    if let Some(l) = limit {
        c.insert(
            SEQUENCED_LIMIT.into(),
            mod_api::Tag::Float(l.max(0.0).to_bits()),
        );
    }
}

/// A saved structure's sequenced limit at `pos`; one there is kept by its next speed check
/// ([`keeps_loaded_limit`]).
fn limit_from_nbt(dim: usize, pos: [i32; 3], c: &Compound) -> Option<f32> {
    let limit = saved_limit(c)?;
    LOADED_LIMITS.with_borrow_mut(|l| l.insert((dim, pos)));
    Some(limit)
}

/// A saved structure's sequenced limit, as [`limit_to_nbt`] wrote it.
fn saved_limit(c: &Compound) -> Option<f32> {
    match c.get(&SEQUENCED_LIMIT.into()) {
        Some(mod_api::Tag::Float(bits)) => Some(f32::from_bits(*bits)),
        _ => None,
    }
}

/// Whether the structure at `pos` was just loaded with a sequenced limit, which its speed
/// check keeps instead of starting the limit again (once).
fn keeps_loaded_limit(dim: usize, pos: [i32; 3]) -> bool {
    LOADED_LIMITS.with_borrow_mut(|l| l.remove(&(dim, pos)))
}

/// Whether the kinetic part at `pos` (a bearing, piston, pulley or gantry shaft) was just placed
/// or loaded and has no speed until its network's first solve: a structure it holds waits for
/// it (Create's block entity loads with its speed, and goes on).
fn unsolved(dim: usize, pos: [i32; 3]) -> bool {
    with_world(|w| w.dims[dim].kinetics.unsolved(pos)).unwrap_or(false)
}

pub(super) fn option_at(dim: usize, pos: [i32; 3]) -> i32 {
    with_world(|w| w.dims[dim].scroll_options.get(&pos).copied())
        .flatten()
        .map_or(0, i32::from)
}

/// A pick on a contraption block's value box (`ScrollOptionBehaviour.setValue` and its
/// callback): a running windmill turns the other way (`onDirectionChanged`), a running clock's
/// hands take their new targets, a roller shares its mode with the rollers in line
/// (`onModeChanged`).
pub fn set_scroll_option(lv: &mut ServerLevel<'_>, pos: [i32; 3], block: Block, value: i32) {
    let dim = lv.dim;
    if block == Block::MechanicalRoller {
        actors::set_roller_mode(lv, pos, value);
        return;
    }
    let value = value.clamp(0, 2) as u8;
    with_world(|w| {
        let d = &mut w.dims[dim];
        if value == 0 {
            d.scroll_options.remove(&pos);
        } else {
            d.scroll_options.insert(pos, value);
        }
    });
    super::persist::mark_pos_unsaved(dim, pos);
    match block {
        Block::WindmillBearing => {
            let state = lv.block(pos);
            with_world(|w| w.windmill_direction(dim, pos, state, value == 0));
        }
        Block::ClockworkBearing => lifts::clock_hands_set(lv, pos, value),
        _ => {}
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

    /// A cell a moving structure passes without colliding (`isCollidingWithWorld`): a block
    /// that can be replaced, or one with no collision shape (and nothing unloaded).
    fn moved_through(&self, pos: [i32; 3]) -> bool {
        self.replaceable(pos)
            || self
                .shape(pos, mod_api::BlockShapeKind::Collision)
                .is_empty()
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
/// Create can't move (unbreakable ones) are unmovable; block entities ride with what they hold
/// (`actors::carry_storage`).
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
    );
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
        attached: attached_sides(&name, &props) | joined_sides(lv.dim, p, &name),
        unsupportive: unsupportive_faces(&name, &props),
        belt: (name == "create:belt")
            .then(|| create_info(id).and_then(|(_, i)| i.belt))
            .flatten(),
    })
}

/// `BlockMovementChecksImpl.isBlockAttachedTowardsFallback` for fluid tanks and item vaults
/// (`ConnectivityHandler.isConnected`): the sides where the block beside is part of the same
/// tank or vault, a `Dir::bit` each.
fn joined_sides(dim: usize, p: [i32; 3], name: &str) -> u8 {
    let tank = name == "create:fluid_tank";
    if !tank && name != "create:item_vault" {
        return 0;
    }
    with_world(|w| {
        let d = &w.dims[dim];
        let of = |q: [i32; 3]| {
            if tank {
                d.tanks.parts.get(&q).map(|part| part.controller)
            } else {
                d.vaults.controller_of(q)
            }
        };
        let Some(own) = of(p) else {
            return 0;
        };
        Dir::ALL.into_iter().fold(0, |acc, dir| {
            if of(add(p, dir.offset())) == Some(own) {
                acc | dir.bit()
            } else {
                acc
            }
        })
    })
    .unwrap_or(0)
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

/// A block state turned `quarter`° about `axis` (`StructureTransform.apply`): its `facing` or
/// `axis` turned with it (`rotateFacing`: `getClockWise(axis)`, `rotation.ordinal()` times);
/// about the vertical, also what vanilla's `BlockState.rotate` turns (a sign's `rotation`, the
/// sides of fences, walls and panes, a rail's `shape`); a half turn about a level axis flips
/// slabs and top or bottom halves; and Create's chassis turn their glued sides
/// (`ITransformableBlock`). A state that has no such turned form stays as it was.
fn turned_state(host: &dyn Host, state: u32, quarter: i32, axis: Axis) -> u32 {
    let turns = create_engine::contraption::clockwise_turns(quarter);
    if turns == 0 {
        return state;
    }
    let Some((name, mut props)) = name_props(host, state) else {
        return state;
    };
    let turn_dir = |d: Dir| (0..turns).fold(d, |d, _| create_engine::wrench::clockwise(d, axis));
    let axis_name = |a: Axis| match a {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    };
    let old_axis = prop(&props, "axis").map(|a| axis_of(Some(a)));
    let turned_axis = old_axis.map(|a| {
        let d = match a {
            Axis::X => Dir::East,
            Axis::Y => Dir::Up,
            Axis::Z => Dir::South,
        };
        turn_dir(d).axis()
    });
    let mut changed = Vec::<(String, String)>::new();
    if let Some(d) = prop(&props, "facing").and_then(dir_of) {
        changed.push(("facing".into(), dir_name(turn_dir(d)).into()));
    } else if let Some(a) = turned_axis {
        changed.push(("axis".into(), axis_name(a).into()));
    }
    // Sides named for directions (fences, walls, panes, vines, glass, chassis glue under
    // their own names below): each takes the value of the side turned into its place.
    let sides: Vec<(Dir, String)> = props
        .iter()
        .filter_map(|(k, v)| dir_of(k).map(|d| (d, v.clone())))
        .collect();
    if !sides.is_empty()
        && sides
            .iter()
            .all(|(d, _)| sides.iter().any(|(e, _)| *e == turn_dir(*d)))
    {
        for (d, v) in &sides {
            changed.push((dir_name(turn_dir(*d)).into(), v.clone()));
        }
    }
    if axis == Axis::Y {
        // `RotationSegment`: sixteen steps, four a quarter turn clockwise.
        if let Some(r) = prop(&props, "rotation").and_then(|r| r.parse::<i32>().ok()) {
            changed.push((
                "rotation".into(),
                ((r + 4 * i32::from(turns)) % 16).to_string(),
            ));
        }
        if let Some(t) = prop(&props, "shape")
            .filter(|_| rail(&name))
            .and_then(|shape| turned_rail(shape, &turn_dir))
        {
            changed.push(("shape".into(), t));
        }
    } else if turns == 2 {
        let flip = |v: &str| match v {
            "top" => Some("bottom"),
            "bottom" => Some("top"),
            _ => None,
        };
        for key in ["type", "half"] {
            if let Some(f) = prop(&props, key).and_then(flip) {
                changed.push((key.into(), f.into()));
            }
        }
    }
    // RadialChassisBlock / LinearChassisBlock `transform`: the glued sides go where they turn.
    if name == "create:radial_chassis"
        && let (Some(from), Some(to)) = (old_axis, turned_axis)
    {
        for d in Dir::ALL {
            if let Some(k) = create_engine::contraption::radial_side(to, d) {
                changed.push((k.into(), "false".into()));
            }
        }
        for d in Dir::ALL {
            let Some(k) = create_engine::contraption::radial_side(from, d) else {
                continue;
            };
            if flag(&props, k)
                && let Some(n) = create_engine::contraption::radial_side(to, turn_dir(d))
            {
                changed.push((n.into(), "true".into()));
            }
        }
    }
    if matches!(
        name.as_str(),
        "create:linear_chassis" | "create:secondary_linear_chassis"
    ) && let Some(from) = old_axis
    {
        let top = match from {
            Axis::X => Dir::East,
            Axis::Y => Dir::Up,
            Axis::Z => Dir::South,
        };
        if !turn_dir(top).positive() {
            let (t, b) = (flag(&props, "sticky_top"), flag(&props, "sticky_bottom"));
            changed.push(("sticky_top".into(), b.to_string()));
            changed.push(("sticky_bottom".into(), t.to_string()));
        }
    }
    if changed.is_empty() {
        return state;
    }
    for (k, v) in changed {
        set_prop(&mut props, &k, &v);
    }
    let p: Vec<(&str, &str)> = props
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    state_with(host, &name, &p).unwrap_or(state)
}

/// `RailShape` turned about the vertical (`BaseRailBlock.rotate`): each end's direction turned.
fn turned_rail(shape: &str, turn: &impl Fn(Dir) -> Dir) -> Option<String> {
    use Dir::{East, North, South, West};
    let ends: (Dir, Dir, bool) = match shape {
        "north_south" => (North, South, false),
        "east_west" => (East, West, false),
        "ascending_north" => (North, South, true),
        "ascending_south" => (South, North, true),
        "ascending_east" => (East, West, true),
        "ascending_west" => (West, East, true),
        "south_east" => (South, East, false),
        "south_west" => (South, West, false),
        "north_west" => (North, West, false),
        "north_east" => (North, East, false),
        _ => return None,
    };
    let (a, b) = (turn(ends.0), turn(ends.1));
    if ends.2 {
        return Some(format!("ascending_{}", dir_name(a)));
    }
    let straight = a.axis() == b.axis();
    Some(if straight {
        if a.axis() == Axis::Z {
            "north_south"
        } else {
            "east_west"
        }
        .to_owned()
    } else {
        let ns = if a.axis() == Axis::Z { a } else { b };
        let ew = if a.axis() == Axis::Z { b } else { a };
        format!("{}_{}", dir_name(ns), dir_name(ew))
    })
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
