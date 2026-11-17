//! Create's processing machines: millstone, press, blaze burner, basin and mixer, drill, saw,
//! crushing wheels, encased fan and nozzle, deployer and mechanical crafter. Their rules are the
//! engine's (`create_engine::{processing, press, basin, ..}`); this module reads the world for
//! them, runs them in the level tick, and writes back: item slots of the block entities
//! (`containers`), item entities, the blocks they break and place.
//!
//! The machines live in the `create:kinetic` block entity type; their slots are declared on it
//! (`containers`). Recipes are read once per data load (`recipes`, on `tags_updated`).
//!
//! The clicks on the machines are the shared `right_click_block`, the
//! wrench's on a deployer and a crafter's edge among them; items landing on a basin or a saw
//! are those blocks' `item_after_vertical_collision`. Some functions here are what other
//! subsystems call: the sequenced assembly steps the spout
//! runs on a belt or depot (`sequenced_*`, `belt_left`), basins' tanks, a stack's sequenced
//! progress. The machines over a belt answer `Subsystem::belt_process`; a crafter's group takes
//! items through its `ITEMS` handler (`crafter::provide_items`), whoever puts them in.
//! The saw, the crushing wheels and the crafter hand their results to whatever takes items from
//! that side (`DirectBeltInputBehaviour`: a belt, depot, funnel, chute, drain or machine) through
//! logistics' `insert_into_from`, which in turn hands belts' items to upward saws, basins,
//! millstones and crushing wheels (`belt_input`). A basin turns its spout toward such a taker
//! diagonally below it (`spoutput`). The basin's, the upward saw's and the deployer's filter
//! slots are `filtering`'s.
//!
//! Left out, the API having no way yet:
//! - A toolbox's one item kind per compartment: `ModContainer` has no per-slot insert check.
//! - The deployer's slower digging of a block whose drops need another tool
//!   (`requiresCorrectToolForDrops` and `isCorrectToolForDrops`): no query for either, so it
//!   digs every block at its harvestable speed (the drops still follow the game's rules).
//! - A hopper putting into a millstone what it can't mill (`MillstoneInventoryHandler`'s
//!   `canProcess`): `ModContainer` has no per-slot insert check.
//! - Create's own damage types (`create:crush`): a native mod's are not registered, so the
//!   crushing wheels deal vanilla's cramming.
//! - The deployer's punch at full strength (`DeployerFakePlayer.getCurrentItemAttackStrengthDelay`
//!   of 1/64): the game's fake player attacks with its own, uncharged, attack strength, so a
//!   punch deals about a fifth of its damage.
//!
//! A blaze burner carried by a moving structure is drawn by the structure's anchor
//! (`burner_draw::draw_moving`): its blaze faces its front rather than the nearest player.

mod air;
mod application;
pub(crate) mod basin;
pub(crate) mod burner_draw;
mod carried;
pub(crate) mod client;
mod containers;
mod crafter;
mod crusher;
mod deployer;
mod fan;
pub(crate) use fan::flow_limit_from;
mod filtering;
mod fist;
pub(crate) use filtering::filter_at as machine_filter;
mod millstone;
mod operator;
mod persist;
mod press;
pub mod recipes;
pub(crate) mod saw;
mod spoutput;
mod stacks;
pub(crate) mod toolbelt;

pub use containers::TOOLBOX_TYPE;
// A deployer riding a contraption (`DeployerMovementBehaviour`) acts and is carried as one in
// the world does.
pub(crate) use crafter::crafter_handler_insert;
pub(crate) use deployer::activate as activate_deployer;
pub(crate) use deployer::set_held_stack as set_deployer_held;
pub(crate) use persist::{deployers_from_nbt, deployers_to_nbt};
pub use press::transitional_of;
// What the other subsystems call: fluids (the spout on a belt
// or depot runs a sequenced assembly's filling step, pumps reach basins' tanks), logistics (a
// stack's sequenced progress onto and off a belt).
pub use basin::basin_tanks;
pub use press::{
    belt_left, sequenced_for, sequenced_on_belt, sequenced_on_depot, sequenced_step_on_depot,
};
pub use stacks::{sequenced_progress, set_sequenced_progress};

use super::server::ServerLevel;
use super::subsystem::Subsystem;
use super::world::with_world;
use create_engine::Block;
use create_engine::kinetics::Dir;
use create_engine::parts::StateInfo;
use mod_api::{
    BlockEntityRef, BlockPart, ClientHost, Compound, HostBlockStates, HostSide,
    ItemAfterVerticalCollisionEvent, PlayerRef, Registrar, RegistrationContext,
    RightClickBlockEvent, ServerHost,
};

pub struct Processing;

impl Subsystem for Processing {
    fn draw_carried(
        &self,
        host: &dyn mod_api::ClientHost,
        block: create_engine::Block,
        event: &mut mod_api::BlockEntitySubmitEvent<'_>,
    ) {
        client::draw_carried(host, block, event);
    }

    fn carry_out(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        block: create_engine::Block,
        out: &mut Compound,
    ) {
        carried::carry_out(lv, pos, block, out);
    }

    fn carry_in(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        block: create_engine::Block,
        data: &Compound,
        turn: create_engine::contraption::Transform,
    ) {
        carried::carry_in(lv, pos, block, data, turn);
    }

    fn animated(
        &self,
        block: create_engine::Block,
        _pos: [i32; 3],
        entity: &mod_api::ModBlockEntity,
    ) -> bool {
        client::animated(block, entity)
    }

    /// A basin's stacks (`entity.items` is empty on a client) and tanks, a mixer's cycle and a
    /// crafter's phase, cover and item, which their clients draw.
    fn synced_keys(&self) -> &'static [&'static str] {
        &[
            basin::ITEMS_KEY,
            basin::FLUIDS_KEY,
            basin::FLUIDS_KEYS_KEY,
            basin::MIXER_KEY,
            basin::GOGGLES_KEY,
            crafter::CRAFTER_KEY,
            saw::SAW_KEY,
            spoutput::VISUALIZED_KEY,
        ]
    }

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let _ = deployer::ITEM_PARTICLE.set(r.particle_key("minecraft:item"));
        containers::register(r);
        recipes::register(r);
        // The crafter's group inventory, ahead of the game's plain slot (it is the kinetic type's,
        // declared by `Kinetics`).
        let kind = r.block_entity_type_key(super::kinetics::TYPE);
        r.block_entity_capability_before_game(&mod_api::ITEMS, &kind, crafter::provide_items);
        client::register(r);
        air::register(r);
        // Entity Part state is offered to native mods only.
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(e) = r.entity_part_with_state("minecraft:horse", fan::Haunted) {
            crate::create_log!("[create] the haunted horse's progress is refused: {e:?}");
        }
        for block in [Block::Basin, Block::MechanicalSaw, Block::Millstone] {
            r.block_part(super::states::key(block), Landing);
        }
        for block in [Block::MechanicalSaw, Block::MechanicalDrill] {
            r.block_part(super::states::key(block), saw::Blades);
        }
        r.block_part(super::states::key(Block::BlazeBurner), BurnerPlacement);
        toolbelt::register(r);
        operator::register(r);
        let mut items = Vec::new();
        for b in Block::ALL
            .iter()
            .copied()
            .filter(|b| toolbelt::toolbox_block(*b))
        {
            r.block_part(super::states::key(b), ToolboxPart);
            let item = r.item_key(&format!("create:{}", b.name()));
            r.item_part(item, ToolboxItem);
            items.push(item);
        }
        let _ = toolbelt::TOOLBOX_ITEMS.set(items);
    }

    fn world_closed(&self) {
        recipes::clear();
        saw::clear();
        press::clear();
        basin::clear();
        spoutput::clear();
        crafter::clear();
        toolbelt::clear();
        fist::clear();
    }

    fn tags_updated(&self, host: &mut dyn ServerHost) {
        recipes::publish(host);
        basin::recipes_changed();
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        persist::load(lv.dim, data);
        toolbelt::load(lv.dim, data);
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        persist::save(lv.dim, chunk, data);
        toolbelt::save(lv.dim, chunk, data);
    }

    fn chunk_unload(&self, dim: usize, chunk: [i32; 2]) {
        toolbelt::chunk_unload(dim, chunk);
    }

    /// A toolbox's block replaced by anything but a toolbox (`ToolboxBlock.onRemove` removing
    /// its block entity): its compartment kinds and id go with it.
    fn block_changed(&self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        let toolbox =
            |s: u32| super::packages::block_of(s).is_some_and(create_engine::dye::is_toolbox);
        if toolbox(old) && !toolbox(new) {
            toolbelt::removed(dim, pos);
        }
    }

    /// Legacy `level_tick_post` order: crafters, millstones, fans, crushers, burners, mixers.
    /// The drills, saws, presses and deployers are `Tools`', after logistics' belts.
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        toolbelt::level_tick();
        crafter::tick_crafters(lv);
        millstone::tick_millstones(lv);
        fan::tick_fans(lv);
        crusher::tick_crushers(lv);
        basin::tick_burners(lv);
        basin::tick_mixers(lv);
        spoutput::tick(lv);
        basin::show_basins(lv);
    }

    /// `BeltProcessingBehaviour` of the machine two above belt segment `segment`
    /// (`BeltSide.process`): a deployer facing down (`BeltDeployerCallbacks`), else a press
    /// (`BeltPressingCallbacks`). A spout's is `fluids`'.
    fn belt_process(
        &self,
        lv: &mut ServerLevel<'_>,
        segment: [i32; 3],
        item: &mut create_engine::belt::Item,
        locked: bool,
    ) -> Option<create_engine::belt::Process> {
        if let Some((d, rpm)) = deployer::deployer_above(lv, segment) {
            return Some(deployer::deployer_on_belt(
                lv, segment, item, locked, d, rpm,
            ));
        }
        press::press_on_belt(lv, segment, item, locked)
    }

    /// `BasinBlock.onWrenched`: the basin's spout side is chosen or shut, not turned.
    fn wrench_used(
        &self,
        lv: &mut ServerLevel<'_>,
        _player: PlayerRef,
        pos: [i32; 3],
        block: Block,
        clicked: Dir,
        _point: [f64; 3],
        sneaking: bool,
    ) -> bool {
        if sneaking {
            return false;
        }
        match block {
            Block::Basin => spoutput::wrenched(lv, pos, clicked),
            // `MechanicalCrafterBlock.onWrenched`: its front face cycles where it points
            // (`state.cycle(POINTING)`: up, left, down, right); any other face passes, so the
            // crafter is never turned.
            Block::MechanicalCrafter => {
                if lv.facing_of(pos, Block::MechanicalCrafter) == Some(clicked) {
                    let next = match lv.property(pos, "pointing").as_deref() {
                        Some("up") => "left",
                        Some("left") => "down",
                        Some("down") => "right",
                        _ => "up",
                    };
                    crate::glue::items::wrench::switch_property(lv, pos, "pointing", next);
                }
            }
            _ => return false,
        }
        true
    }

    fn player_joined(&self, host: &mut dyn ServerHost, player: PlayerRef) {
        press::player_joined(host, player);
        deployer::player_joined(host, player);
        toolbelt::player_joined(host, player);
        fan::player_joined(player);
    }

    fn server_message(&self, host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        if channel == air::CHANNEL {
            air::from_server(data);
            return true;
        }
        if channel == air::FLYING_CHANNEL {
            air::flying_from_server(data);
            return true;
        }
        toolbelt::server_message(host, channel, data) || client::server_message(host, channel, data)
    }

    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        if channel != toolbelt::CHANNEL {
            return false;
        }
        toolbelt::client_message(host, player, data);
        true
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        client::tick(host);
        toolbelt::client_tick(host);
        air::tick(host);
    }

    /// The fans' push on the local player, at the start of its tick (its client moves it).
    fn player_tick_pre(&self, host: HostSide, event: &mut mod_api::PlayerTickPreEvent) {
        if let HostSide::Client(host) = host {
            air::push_player(host, event.player);
        }
    }

    fn particle_providers(&self, providers: &mut dyn mod_api::ParticleProviders) {
        providers.register(air::AIR_FLOW.to_owned(), std::sync::Arc::new(air::AirFlow));
    }

    fn client_reset(&self) {
        air::reset();
        client::reset();
        toolbelt::client_reset();
    }

    fn player_tick(&self, host: HostSide, event: &mut mod_api::PlayerTickPostEvent) {
        if let HostSide::Server(host) = host {
            toolbelt::player_tick(host, event.player);
            fan::player_tick(host, event.player);
        }
    }

    /// `ToolboxBlock.attack`: a left click picks a toolbox up.
    fn left_click_block(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::LeftClickBlockEvent,
    ) -> mod_api::Flow {
        toolbelt::left_click(host, event.pos)
    }

    /// `ToolboxHandlerClient.onPickItem`: the picked item from a toolbox in reach.
    fn player_pick_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::PlayerPickItemEvent<'_>,
    ) -> mod_api::Flow {
        toolbelt::pick_item(host, event)
    }

    /// A player breaking a toolbox: its contents go with its drop.
    fn break_block(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::BreakBlockEvent<'_>,
    ) -> mod_api::Flow {
        if event.cancelled {
            return mod_api::Flow::Continue;
        }
        let Some(player) = host.players().acting_player() else {
            return mod_api::Flow::Continue;
        };
        let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
            return mod_api::Flow::Continue;
        };
        if let Some(mut lv) = ServerLevel::new(host, &dimension) {
            toolbelt::before_removal(&mut lv, event.pos);
        }
        mod_api::Flow::Continue
    }

    fn block_drops(
        &self,
        level: &dyn mod_api::LevelHost,
        event: &mut mod_api::BlockDropsEvent<'_>,
    ) -> mod_api::Flow {
        toolbelt::block_drops(level, event);
        mod_api::Flow::Continue
    }

    fn render_gui_post(&self, host: &dyn ClientHost, event: &mut mod_api::RenderGuiPostEvent<'_>) {
        toolbelt::render_gui_post(host, event);
    }

    fn screen_render_post(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenRenderPostEvent<'_>,
    ) {
        toolbelt::screen_render_post(host, event);
    }

    fn screen_mouse_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenMouseButtonPressedPreEvent<'_>,
    ) -> mod_api::Flow {
        toolbelt::screen_mouse_pressed(host, event)
    }

    fn screen_mouse_scrolled(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenMouseScrolledPreEvent<'_>,
    ) -> mod_api::Flow {
        toolbelt::screen_mouse_scrolled(host, event)
    }

    fn screen_key_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenKeyPressedPreEvent<'_>,
    ) -> mod_api::Flow {
        toolbelt::screen_key_pressed(host, event)
    }

    fn screen_closing(&self, host: &dyn ClientHost, event: &mut mod_api::ScreenClosingEvent<'_>) {
        toolbelt::screen_closing(host, event);
    }

    fn screen_key_released(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenKeyReleasedPreEvent<'_>,
    ) -> mod_api::Flow {
        toolbelt::screen_key_released(host, event)
    }

    fn standalone_models(&self, models: &mut Vec<String>) {
        models.push(burner_draw::FLAME_MODEL.to_owned());
        models.push(burner_draw::FLAME_SCROLLS_MODEL.to_owned());
    }

    fn models_baked(&self, host: &mut dyn ClientHost) {
        burner_draw::models_baked(host);
    }

    fn right_click_block(
        &self,
        host: HostSide,
        event: &mut RightClickBlockEvent<'_>,
    ) -> mod_api::Flow {
        match host {
            HostSide::Server(host) => {
                if application::apply_in_world(host, event) {
                    return mod_api::Flow::Cancel;
                }
                use_block(host, event)
            }
            HostSide::Client(host) => {
                let click = event.click;
                let block = host
                    .level(host.dimension())
                    .and_then(|l| l.block_state(&*host, click.pos))
                    .and_then(super::states::id)
                    .and_then(|id| {
                        super::client_table(&*host, |t| t.get(id).map(|(b, _)| b)).flatten()
                    });
                if let Some(block) = block
                    && client::filter_slot_click(&*host, block, click)
                {
                    *event.cancellation_result = mod_api::InteractionResult::Success;
                    return mod_api::Flow::Cancel;
                }
                if let Some(block) = block
                    && passes_item_use(&*host, block, click.stack)
                {
                    *event.use_block = mod_api::TriState::False;
                }
                mod_api::Flow::Continue
            }
        }
    }
}

/// A toolbox exploding: its contents go with its drop.
struct ToolboxPart;

#[mod_api::horizon_part]
impl BlockPart for ToolboxPart {
    fn on_block_exploded(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::OnBlockExplodedEvent<'_>,
    ) {
        let dimension = event.explosion.dimension.to_owned();
        if let Some(mut lv) = ServerLevel::new(host, &dimension) {
            toolbelt::before_removal(&mut lv, [event.pos.x, event.pos.y, event.pos.z]);
        }
    }
}

/// A toolbox placed from its item takes the contents the item keeps.
struct ToolboxItem;

#[mod_api::horizon_part]
impl mod_api::ItemPart for ToolboxItem {
    fn on_block_placed_by(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::OnBlockPlacedByEvent<'_>,
    ) {
        let Some(player) = event.placer else { return };
        let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
            return;
        };
        let pos = event.pos;
        let stack = event.stack.clone();
        if let Some(mut lv) = ServerLevel::new(host, &dimension) {
            toolbelt::placed(&mut lv, pos, &stack);
        }
    }
}

/// Whether a click with `stack` on `block` passes to the item's own use (a block placed against
/// it), the block taking no part: `PASS_TO_DEFAULT_BLOCK_INTERACTION` and no `useWithoutItem`.
/// These blocks are templated on the barrel for their slots, whose own use would take the click.
fn passes_item_use(
    host: &dyn mod_api::Host,
    block: Block,
    stack: Option<&mod_api::ItemStack>,
) -> bool {
    let Some(stack) = stack else {
        return false;
    };
    match block {
        Block::Millstone => true,
        // `BasinBlock.useItemOn`: what isn't a fluid container (only buckets here).
        Block::Basin => {
            let id = stacks::id(host, stack);
            id != "minecraft:bucket" && create_engine::tank::Fluid::of_bucket(&id).is_none()
        }
        _ => false,
    }
}

/// The drills and saws (after the vaults and chutes), then the presses and
/// deployers, which work the depots and belts logistics moved this tick (`DepotBehaviour.tick`
/// before `PressingBehaviour.tick`).
pub struct Tools;

impl Subsystem for Tools {
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        saw::tick_drills(lv);
        saw::tick_saws(lv);
        saw::hurt_inside(lv);
        press::tick_presses(lv);
        deployer::tick_deployers(lv);
    }
}

/// The basin, the saw and the millstone taking an item that lands on them
/// (`updateEntityAfterFallOn`): a basin puts it into its inputs, a saw
/// facing up takes it to cut, a millstone into its input when it can mill it. Stacks with
/// components stay entities, but on a saw a sequenced assembly's transitional item (its
/// progress with it).
struct Landing;

#[mod_api::horizon_part]
impl BlockPart for Landing {
    fn item_after_vertical_collision(
        &self,
        host: HostSide,
        event: &mut ItemAfterVerticalCollisionEvent<'_>,
    ) -> mod_api::Flow {
        let HostSide::Server(host) = host else {
            return mod_api::Flow::Continue;
        };
        let Some(mut lv) = ServerLevel::new(host, event.dimension) else {
            return mod_api::Flow::Continue;
        };
        let pos = event.block_pos;
        let Some(stack) = event.stack.as_ref() else {
            return mod_api::Flow::Continue;
        };
        let block = lv.info(pos).map(|(b, _)| b);
        let carried = block == Some(Block::MechanicalSaw)
            && stack.damage() == 0
            && stacks::sequenced_progress(stack).is_some();
        if (stack.components().is_some() || stack.damage() != 0) && !carried {
            return mod_api::Flow::Continue;
        }
        match block {
            Some(Block::Basin) => basin::basin_take(&mut lv, pos, event.stack),
            Some(Block::MechanicalSaw) => saw::saw_take(&mut lv, pos, event.stack),
            Some(Block::Millstone) => millstone::millstone_take(&mut lv, pos, event.stack),
            _ => {}
        }
        mod_api::Flow::Continue
    }
}

/// `BlazeBurnerBlock.getStateForPlacement` for its own item (the empty burner's is decor's):
/// a captured blaze, smouldering, facing the player.
struct BurnerPlacement;

#[mod_api::horizon_part]
impl BlockPart for BurnerPlacement {
    fn state_for_placement(
        &self,
        level: &dyn mod_api::LevelHost,
        event: &mut mod_api::StateForPlacementEvent<'_>,
    ) -> Option<Option<mod_api::BlockState>> {
        let c = event.context;
        let facing = create_engine::decor::horizontal_facing(c.yaw).opposite();
        let state = level.state_with_property(event.state, "blaze", "smouldering")?;
        let state =
            level.state_with_property(state, "facing", super::kinetics::dir_name(facing))?;
        Some(Some(state))
    }
}

/// `useItemOn` of the machines a player clicks: the millstone, the
/// mechanical crafter, the deployer and the blaze burner. Cancelled as used when it took the
/// click.
fn use_block(host: &mut dyn ServerHost, event: &mut RightClickBlockEvent<'_>) -> mod_api::Flow {
    let click = event.click;
    let dimension = click.dimension.key().id().to_owned();
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return mod_api::Flow::Continue;
    };
    let Some(player) = lv.host.player(click.player) else {
        return mod_api::Flow::Continue;
    };
    if player.entity().spectator(&*lv.host) == Some(true) {
        return mod_api::Flow::Continue;
    }
    let pos = click.pos;
    let Some((block, info)) = lv.info(pos) else {
        return mod_api::Flow::Continue;
    };
    let main = click.hand == mod_api::Hand::Main;
    // A filter slot takes the click first (`ValueSettingsInputHandler`).
    if matches!(block, Block::Basin | Block::MechanicalSaw | Block::Deployer)
        && filtering::click(&mut lv, pos, click)
    {
        *event.cancellation_result = mod_api::InteractionResult::Success;
        return mod_api::Flow::Cancel;
    }
    // The wrench (`WrenchItem.useOn`, not sneaking): a deployer's front half changes its mode
    // (`onWrenched`), a crafter's edge joins or parts its input (`EdgeInteractionHandler`).
    let wrench = click
        .stack
        .is_some_and(|s| stacks::id(&*lv.host, s) == "create:wrench");
    let sneaking = player.entity().is_shift_key_down(&*lv.host) == Some(true);
    if wrench && !sneaking {
        let wrenched = match block {
            Block::Deployer => deployer::wrench_deployer(&mut lv, pos, click.hit.point),
            Block::MechanicalCrafter => Dir::from_offset(click.hit.face)
                .is_some_and(|face| crafter::crafter_edge(&mut lv, pos, face, click.hit.point)),
            _ => false,
        };
        if wrenched {
            *event.cancellation_result = mod_api::InteractionResult::Success;
            return mod_api::Flow::Cancel;
        }
    }
    let used = match block {
        Block::Millstone => millstone::use_millstone(&mut lv, pos, player, click.stack),
        Block::MechanicalCrafter => crafter::use_crafter(&mut lv, pos, click, player, main),
        Block::Deployer => info
            .facing
            .is_some_and(|f| deployer::use_deployer(&mut lv, pos, f, click, player, main)),
        Block::BlazeBurner => basin::use_burner(&mut lv, pos, player, click.hand, click.stack),
        Block::LitBlazeBurner => {
            basin::use_lit_burner(&mut lv, pos, player, click.hand, click.stack)
        }
        Block::Basin if main => basin::use_basin(&mut lv, pos, player, click.stack),
        _ => false,
    };
    if used {
        *event.cancellation_result = mod_api::InteractionResult::Success;
        return mod_api::Flow::Cancel;
    }
    if passes_item_use(&*lv.host, block, click.stack) {
        *event.use_block = mod_api::TriState::False;
    }
    mod_api::Flow::Continue
}

/// What the processing glue reads of one level.
impl ServerLevel<'_> {
    /// The Create block at `pos` and its state's facts.
    pub fn info(&self, pos: [i32; 3]) -> Option<(Block, StateInfo)> {
        let id = self.block(pos);
        if id == 0 {
            return None;
        }
        with_world(|w| w.table.get(id).map(|(b, i)| (b, *i))).flatten()
    }

    /// The facing of the Create block `block` at `pos`; None for any other block.
    pub fn facing_of(&self, pos: [i32; 3], block: Block) -> Option<Dir> {
        self.info(pos).filter(|(b, _)| *b == block)?.1.facing
    }

    /// The block entity at `pos`.
    pub fn entity_at(&self, pos: [i32; 3]) -> Option<BlockEntityRef> {
        self.host.block_entity(self.level, pos)
    }

    /// Whether the block at `p` can be replaced (`canBeReplaced`: air, a fluid, grass).
    pub fn replaceable(&self, p: [i32; 3]) -> bool {
        self.block_state(p)
            .and_then(|s| self.host.block_states().replaceable(s))
            .unwrap_or(false)
    }

    /// Whether the block at `p` is air.
    pub fn is_air(&self, p: [i32; 3]) -> bool {
        matches!(
            self.block_name(p).as_deref(),
            Some("minecraft:air" | "minecraft:cave_air" | "minecraft:void_air") | None
        )
    }

    /// Whether the block at `p` is in the block tag `tag`.
    pub fn block_tagged(&self, p: [i32; 3], tag: &str) -> bool {
        self.level
            .block(&*self.host, p)
            .and_then(|k| self.host.block_in_tag(k, tag))
            .unwrap_or(false)
    }

    /// A property of the block at `p`.
    pub fn property(&self, p: [i32; 3], key: &str) -> Option<String> {
        self.level.block_property(&*self.host, p, key)
    }

    /// The boxes of the block's shape at `p` (`kind`), in blocks from its corner.
    pub fn shape(&self, p: [i32; 3], kind: mod_api::BlockShapeKind) -> Vec<[f64; 6]> {
        self.block_state(p)
            .and_then(|s| self.level.block_shape(&*self.host, s, p, kind))
            .unwrap_or_default()
    }

    /// `BeltProcessingBehaviour.isBlocked`: a block with a collision shape at `at`.
    pub fn blocked(&self, at: [i32; 3]) -> bool {
        !self
            .shape(at, mod_api::BlockShapeKind::Collision)
            .is_empty()
    }

    /// The top of the outline of the block at `p`, 0 to 1 (`getShape(..).max(Y)`).
    pub fn outline_top(&self, p: [i32; 3]) -> f64 {
        self.shape(p, mod_api::BlockShapeKind::Outline)
            .iter()
            .map(|b| b[4])
            .fold(0.0, f64::max)
    }

    /// The speed of the kinetic part at `pos`.
    pub fn speed(&self, pos: [i32; 3]) -> f32 {
        let dim = self.dim;
        with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0)
    }

    /// A seed for this tick's rolls at `pos`.
    pub fn seed_at(&self, pos: [i32; 3]) -> u64 {
        self.game_time() as u64 ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9) ^ (pos[2] as u64) << 20
    }
}

/// Whether a processing machine at `pos` takes items from belts (`DirectBeltInputBehaviour`):
/// Some(None) from any side, Some(Some(d)) only for items moving toward `d` (a crushing wheel
/// controller between wheels above each other: `supportsDirectBeltInput`).
pub(crate) fn belt_input_side(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Option<Dir>> {
    let (block, info) = lv.info(pos)?;
    match block {
        Block::MechanicalSaw | Block::Basin | Block::Millstone => Some(None),
        Block::CrushingWheelController => Some(info.facing.filter(|&f| f != Dir::Down)),
        _ => None,
    }
}

/// `DirectBeltInputBehaviour.handleInsertion` of the processing machines at `next`, for `item`
/// moving toward `side` (off a belt's end, out of a saw or crusher): an upward saw, a basin, a
/// millstone or a crushing wheel controller, through their item handlers (the default
/// insertion callback). What is left of it; None when no such machine is there.
pub(crate) fn belt_input(
    lv: &mut ServerLevel<'_>,
    next: [i32; 3],
    side: Dir,
    item: &create_engine::belt::Item,
) -> Option<u8> {
    belt_input_sim(lv, next, side, item, false)
}

/// `belt_input`, or (`simulate`) what it would leave without taking any.
pub(crate) fn belt_input_sim(
    lv: &mut ServerLevel<'_>,
    next: [i32; 3],
    side: Dir,
    item: &create_engine::belt::Item,
    simulate: bool,
) -> Option<u8> {
    let (block, info) = lv.info(next)?;
    if simulate {
        belt_input_side(lv, next)?;
        return Some(belt_input_room(lv, next, block, info, side, item));
    }
    let count = item.count;
    let stack = || stacks::stack(&*lv.host, &item.item, u32::from(count));
    let left = |s: &Option<mod_api::ItemStack>| s.as_ref().map_or(0, |s| s.count().min(255) as u8);
    match block {
        // The saw's inventory is offered on every side but its bottom; it takes one item
        // while empty (`ProcessingInventory`, a slot limit of one without bulk cutting).
        Block::MechanicalSaw => {
            if info.facing != Some(Dir::Up) || side == Dir::Down {
                return Some(count);
            }
            Some(
                if saw::saw_put(lv, next, &item.item, item.progress.clone()) {
                    count - 1
                } else {
                    count
                },
            )
        }
        Block::Basin => {
            let mut s = stack();
            basin::basin_take(lv, next, &mut s);
            Some(left(&s))
        }
        Block::Millstone => {
            let mut s = stack();
            millstone::millstone_take(lv, next, &mut s);
            Some(left(&s))
        }
        // `supportsDirectBeltInput`: a controller between wheels side by side takes from any
        // side, one between wheels above each other only from the side it faces.
        Block::CrushingWheelController => {
            if info.facing != Some(Dir::Down) && info.facing != Some(side) {
                return Some(count);
            }
            Some(if crusher::crusher_put(lv, next, &item.item, count) {
                0
            } else {
                count
            })
        }
        _ => None,
    }
}

/// What `belt_input` would leave of `item` at the processing machine `block` at `pos`.
fn belt_input_room(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    block: Block,
    info: create_engine::parts::StateInfo,
    side: Dir,
    item: &create_engine::belt::Item,
) -> u8 {
    let count = item.count;
    let dim = lv.dim;
    let stacked = |lv: &ServerLevel<'_>, slots: std::ops::Range<usize>| {
        let max = stacks::limit(&*lv.host, &item.item);
        let Some(mut inputs) = lv.read_slots(pos, slots.start, slots.end) else {
            return count;
        };
        create_engine::processing::insert_stacked(&mut inputs, &item.item, count, max)
    };
    match block {
        Block::MechanicalSaw => {
            let empty = with_world(|w| {
                w.dims[dim]
                    .saws
                    .get(&pos)
                    .is_none_or(|s| s.slots.is_empty())
            })
            .unwrap_or(false);
            if info.facing != Some(Dir::Up) || side == Dir::Down || !empty {
                count
            } else {
                count - 1
            }
        }
        Block::Basin => stacked(lv, create_engine::basin::INPUTS),
        Block::Millstone => {
            let mills = recipes::with(|r| r.milling.find_single(&item.item))
                .flatten()
                .is_some();
            if mills { stacked(lv, 0..1) } else { count }
        }
        Block::CrushingWheelController => {
            if info.facing != Some(Dir::Down) && info.facing != Some(side) {
                return count;
            }
            let free = with_world(|w| {
                w.dims[dim]
                    .crushers
                    .get(&pos)
                    .is_some_and(|(c, _)| !c.occupied())
            })
            .unwrap_or(false);
            if free { 0 } else { count }
        }
        _ => count,
    }
}

/// Whether something at `p` takes items handed to it moving toward `side`
/// (`DirectBeltInputBehaviour` there, and its `canInsertFromSide(side)`): a belt, depot, funnel,
/// chute, item drain or processing machine.
pub(crate) fn direct_input(lv: &ServerLevel<'_>, p: [i32; 3], side: Dir) -> bool {
    crate::glue::logistics::direct_input_at(lv, p)
        && crate::glue::logistics::direct_input_accepts(lv, p, side)
}

/// Whether a depot or a belt is at `p` (`DirectBeltInputBehaviour` there).
pub(crate) fn takes_from_side(dim: usize, p: [i32; 3]) -> bool {
    with_world(|w| {
        let d = &w.dims[dim];
        d.depots.contains_key(&p) || d.belt_of.contains_key(&p)
    }) == Some(true)
}

/// `pos` moved `n` along `d`.
pub(crate) fn step(pos: [i32; 3], d: Dir, n: i32) -> [i32; 3] {
    let o = d.offset();
    [pos[0] + o[0] * n, pos[1] + o[1] * n, pos[2] + o[2] * n]
}
