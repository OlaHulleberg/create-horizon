//! Create's processing machines: millstone, press, blaze burner, basin and mixer, drill, saw,
//! crushing wheels, encased fan and nozzle, deployer and mechanical crafter. Their rules are the
//! engine's (`create_engine::{processing, press, basin, ..}`); this module reads the world for
//! them, runs them in the level tick, and writes back: item slots of the block entities
//! (`containers`), item entities, the blocks they break and place.
//!
//! The machines live in the `create:kinetic` block entity type; their slots are declared on it
//! (`containers`). Recipes are read once per data load (`recipes`, on `tags_updated`).
//!
//! The clicks on the machines are the shared `right_click_block` (as in the legacy glue), the
//! wrench's on a deployer and a crafter's edge among them; items landing on a basin or a saw
//! are those blocks' `item_after_vertical_collision`. Some functions here are what other
//! subsystems call, as the legacy glue's siblings did: the sequenced assembly steps the spout
//! runs on a belt or depot (`sequenced_*`, `belt_left`), basins' tanks, a stack's sequenced
//! progress. The machines over a belt answer `Subsystem::belt_process`; a crafter's group takes
//! items through its `ITEMS` handler (`crafter::provide_items`, the legacy
//! `crafter_handler_insert`), whoever puts them in.
//! The saw, the crushing wheels and the crafter hand their results to a belt or depot beside
//! them through logistics' `insert_into_from`, as in the legacy glue.
//!
//! Left out, the API having no way yet:
//! - A toolbox's one item kind per compartment: `ModContainer` has no per-slot insert check
//!   (left out in the legacy glue too).
//! - The deployer's slower digging of a block whose drops need another tool
//!   (`requiresCorrectToolForDrops` and `isCorrectToolForDrops`): no query for either, so it
//!   digs every block at its harvestable speed (the drops still follow the game's rules).
//! - A hopper putting into a millstone what it can't mill (`MillstoneInventoryHandler`'s
//!   `canProcess`): `ModContainer` has no per-slot insert check.
//! - Create's own damage types (`create:crush`): a native mod's are not registered, so the
//!   crushing wheels deal vanilla's cramming.
//!
//! A blaze burner's blaze, rods, flame and goggles are its block entity's drawing
//! (`burner_draw`), so a burner carried by a moving structure, drawn from its block model,
//! shows only its cage.
//!
//! Not drawn yet: the items a saw facing up is cutting (`SawRenderer.renderItems`) and a grid's
//! items moving between mechanical crafters (`MechanicalCrafterRenderer.renderItems` past
//! `IDLE`).

mod application;
pub(crate) mod basin;
mod burner_draw;
mod client;
mod containers;
mod crafter;
mod crusher;
mod deployer;
mod fan;
mod millstone;
mod persist;
mod press;
pub mod recipes;
pub(crate) mod saw;
mod stacks;

pub use containers::TOOLBOX_TYPE;
pub use press::transitional_of;
// What the other subsystems call (the legacy glue's siblings did): fluids (the spout on a belt
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
            basin::MIXER_KEY,
            basin::GOGGLES_KEY,
            crafter::CRAFTER_KEY,
        ]
    }

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        containers::register(r);
        // The crafter's group inventory, ahead of the game's plain slot (it is the kinetic type's,
        // declared by `Kinetics`).
        let kind = r.block_entity_type_key(super::kinetics::TYPE);
        r.block_entity_capability_before_game(&mod_api::ITEMS, &kind, crafter::provide_items);
        client::register(r);
        for block in [Block::Basin, Block::MechanicalSaw, Block::Millstone] {
            r.block_part(super::states::key(block), Landing);
        }
        r.block_part(super::states::key(Block::BlazeBurner), BurnerPlacement);
    }

    fn world_closed(&self) {
        recipes::clear();
        saw::clear();
        basin::clear();
    }

    fn tags_updated(&self, host: &mut dyn ServerHost) {
        recipes::publish(host);
        basin::recipes_changed();
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        persist::load(lv.dim, data);
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        persist::save(lv.dim, chunk, data);
    }

    /// Legacy `level_tick_post` order: crafters, millstones, fans, crushers, burners, mixers.
    /// The drills, saws, presses and deployers are `Tools`', after logistics' belts.
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        crafter::tick_crafters(lv);
        millstone::tick_millstones(lv);
        fan::tick_fans(lv);
        crusher::tick_crushers(lv);
        basin::tick_burners(lv);
        basin::tick_mixers(lv);
        basin::show_basins(lv);
    }

    /// `BeltProcessingBehaviour` of the machine two above belt segment `segment` (the legacy
    /// `BeltSide.process`): a deployer facing down (`BeltDeployerCallbacks`), else a press
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

    fn player_joined(&self, host: &mut dyn ServerHost, player: PlayerRef) {
        press::player_joined(host, player);
        deployer::player_joined(host, player);
    }

    fn server_message(&self, host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        client::server_message(host, channel, data)
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        client::tick(host);
    }

    fn client_reset(&self) {
        client::reset();
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
                    && passes_item_use(&*host, block, click.stack)
                {
                    *event.use_block = mod_api::TriState::False;
                }
                mod_api::Flow::Continue
            }
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

/// The drills and saws (after the vaults and chutes in the legacy order), then the presses and
/// deployers, which work the depots and belts logistics moved this tick (`DepotBehaviour.tick`
/// before `PressingBehaviour.tick`, as the legacy `tick_depots`, `tick_belts`, `tick_presses`).
pub struct Tools;

impl Subsystem for Tools {
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        saw::tick_drills(lv);
        saw::tick_saws(lv);
        press::tick_presses(lv);
        deployer::tick_deployers(lv);
    }
}

/// The basin, the saw and the millstone taking an item that lands on them
/// (`updateEntityAfterFallOn`, legacy `item_landed`): a basin puts it into its inputs, a saw
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

/// `useItemOn` of the machines a player clicks (legacy `right_click_block`): the millstone, the
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
        Block::BlazeBurner => basin::use_burner(&mut lv, pos, player, click.stack),
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

    /// A seed for this tick's rolls at `pos` (the legacy glue's).
    pub fn seed_at(&self, pos: [i32; 3]) -> u64 {
        self.game_time() as u64 ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9) ^ (pos[2] as u64) << 20
    }
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
