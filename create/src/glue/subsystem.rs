//! What each part of the glue (kinetics, logistics, processing...) takes part in.
//!
//! `lib.rs` implements each hook once and hands it to every subsystem in `SUBSYSTEMS` order,
//! which is also the order their level ticks run in (Create's own: block entities in the order
//! of the systems that hold them). A subsystem implements only what it needs; the rest are the
//! default no-ops. Adding a moment means adding a method here and its one call in `lib.rs`.
//!
//! The global hooks several subsystems share take the hook's own host and event, so a
//! subsystem reads them as the API documents them: `lib.rs` calls each subsystem in order and
//! stops at the first `Flow::Cancel` (or the first `Some` for a replacement). A moment that
//! belongs to one block, item or block entity is not here: the subsystem attaches its own Part
//! to its own blocks in `register` (a block may carry several Parts).

use super::server::ServerLevel;
use mod_api::{
    AttackEntityEvent, BreakBlockEvent, CameraPoseEvent, ClientHost, Compound, EntityInteractEvent,
    EntityJoinLevelEvent, EntityMountEvent, EntityPlaceEvent, Flow, HostSide, InputKeyEvent,
    InteractionKeyMappingTriggeredEvent, InteractionResult, KeyMappingPreEvent,
    LeftClickBlockEvent, LevelSaveEvent, MenuClosedEvent, MouseScrollingEvent, MovementInputEvent,
    PlayerContainerCloseEvent, PlayerContainerOpenEvent, PlayerRef, PlayerTickPostEvent,
    RegisterCommandsEvent, Registrar, RegistrationContext, RenderGuiPostEvent,
    RightClickBlockEvent, RightClickItemEvent, RunModCommandEvent, ScreenCharTypedPreEvent,
    ScreenClosingEvent, ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent,
    ScreenMouseButtonReleasedPreEvent, ScreenMouseScrolledPreEvent, ScreenRenderPostEvent,
    ServerHost,
};

pub trait Subsystem: Sync {
    /// Its registrations: parts, block entity types, payloads, commands. Once per game.
    fn register(&self, _r: &mut dyn Registrar, _cx: &RegistrationContext) {}

    /// Create's world opened on this server thread (the engine's world exists).
    fn world_opened(&self, _host: &mut dyn ServerHost) {}

    /// The world closed: drop everything kept for it.
    fn world_closed(&self) {}

    /// `Hooks::tags_updated` on the server: its recipes and tags are published (first open and
    /// each data reload). Caches of recipes are rebuilt here.
    fn tags_updated(&self, _host: &mut dyn ServerHost) {}

    /// A committed block change in level `dim`, in engine ids (0 for any block not Create's).
    /// At least one of `old` and `new` is Create's. Runs inside the write: change the world from
    /// the level tick, not here.
    fn block_changed(&self, _dim: usize, _pos: [i32; 3], _old: u32, _new: u32) {}

    /// A neighbour of the Create block `state` at `pos` changed. Inside the write, as above.
    fn neighbor_changed(&self, _dim: usize, _pos: [i32; 3], _state: u32) {}

    /// A column came into level `lv` with what was saved for it (`chunk_data_save`), before
    /// its blocks are found (their block entities load after).
    fn chunk_load(&self, _lv: &mut ServerLevel<'_>, _chunk: [i32; 2], _data: &Compound) {}

    /// What to keep for column `chunk` of level `dim`: written into the column's attachment.
    fn chunk_data_save(&self, _lv: &ServerLevel<'_>, _chunk: [i32; 2], _data: &mut Compound) {}

    /// A column left level `dim`.
    fn chunk_unload(&self, _dim: usize, _chunk: [i32; 2]) {}

    /// A moving structure takes Create block `block` at `pos` away (`Contraption.addBlock`
    /// keeps its block entity's NBT in `StructureBlockInfo.nbt`): what this subsystem keeps for
    /// it goes into `out` under the subsystem's own keys and leaves its tables, so the block's
    /// removal drops nothing. Called before the block leaves the world.
    fn carry_out(
        &self,
        _lv: &mut ServerLevel<'_>,
        _pos: [i32; 3],
        _block: create_engine::Block,
        _out: &mut Compound,
    ) {
    }

    /// Draws what a block entity riding a moving structure holds, under the structure's pose
    /// at the block (`BlockEntityRenderHelper.renderBlockEntities` with the contraption's update
    /// tags): `event.entity.nbt` is the block entity's data as the structure took it (its synced
    /// keys, its stacks under `Items`, and what [`carry_out`](Self::carry_out) wrote under
    /// `Carried`); `event.pos` keys nothing in the world (it stands for the block in the
    /// subsystem's own client tables), and `keep` is the structure's to call.
    fn draw_carried(
        &self,
        _host: &dyn ClientHost,
        _block: create_engine::Block,
        _event: &mut mod_api::BlockEntitySubmitEvent<'_>,
    ) {
    }

    /// The block [`carry_out`](Self::carry_out) took goes back at `pos`, turned by `turn`
    /// (`addBlocksToWorld` loads the NBT into the block entity placed there, and
    /// `TransformableBlockEntity.transform` turns what points somewhere): what it wrote, in
    /// `data`. Called just before the block is placed, as a column's saved data is read before
    /// its blocks are found, so the block takes it as it would from a save (a moving structure
    /// saved with the world keeps `data`).
    fn carry_in(
        &self,
        _lv: &mut ServerLevel<'_>,
        _pos: [i32; 3],
        _block: create_engine::Block,
        _data: &Compound,
        _turn: create_engine::contraption::Transform,
    ) {
    }

    /// The part at `pos` of `lv` turns at a new speed (`KineticBlockEntity.onSpeedChanged`), in
    /// this tick's solve.
    fn speed_changed(&self, _lv: &mut ServerLevel<'_>, _pos: [i32; 3], _speed: f32) {}

    /// The keys of a `create:kinetic` block entity's data this subsystem keeps for clients
    /// (sent with the parts' speeds in its update tag).
    fn synced_keys(&self) -> &'static [&'static str] {
        &[]
    }

    /// Whether this subsystem draws part `part` of `block` itself, so the kinetic renderer
    /// leaves it out (a gauge's dial, a flywheel's easing wheel).
    fn draws_itself(
        &self,
        _block: create_engine::Block,
        _part: &create_engine::client::Draw,
    ) -> bool {
        false
    }

    /// Whether this subsystem animates the `create:kinetic` block entity at `pos` this frame
    /// (a moving arm, a sliding structure): its drawing is then not kept, and every part on it is
    /// drawn again next frame.
    fn animated(
        &self,
        _block: create_engine::Block,
        _pos: [i32; 3],
        _entity: &mod_api::ModBlockEntity,
    ) -> bool {
        false
    }

    /// After a server level's tick.
    fn level_tick(&self, _lv: &mut ServerLevel<'_>) {}

    /// Publish live entity state after every subsystem has finished mutating the level.
    /// Like Create's BlockEntity.write, this must observe the final state, including
    /// transfers from subsystems that run later than the entity's own tick.
    fn write_entities(&self, _lv: &mut ServerLevel<'_>) {}

    /// Whether `level_tick` runs while the server is frozen (`/tick freeze`): only for what
    /// Create ticks from `LevelTickEvent` rather than from block entities.
    fn runs_frozen(&self) -> bool {
        false
    }

    /// A world fluid's scheduled tick is about to run (`FlowingFluid.tick`): set `cancelled` to
    /// skip it. Read-only: what changes is done from the next `level_tick`.
    fn fluid_tick(
        &self,
        _level: &dyn mod_api::LevelHost,
        _event: &mut mod_api::FluidTickEvent<'_>,
    ) {
    }

    /// An item on the belt segment at `segment` reaches its middle (`locked` false:
    /// `BeltProcessingBehaviour.handleReceivedItem`) or is held there (`handleHeldItem`): what
    /// a machine above it does. The first subsystem to answer acts; None when it has none there.
    fn belt_process(
        &self,
        _lv: &mut ServerLevel<'_>,
        _segment: [i32; 3],
        _item: &mut create_engine::belt::Item,
        _locked: bool,
    ) -> Option<create_engine::belt::Process> {
        None
    }

    /// A player joined the server.
    fn player_joined(&self, _host: &mut dyn ServerHost, _player: PlayerRef) {}

    /// Bytes on one of Create's channels from a player's client. True when this subsystem owns
    /// `channel` (the others are not asked).
    fn client_message(
        &self,
        _host: &mut dyn ServerHost,
        _player: PlayerRef,
        _channel: &str,
        _data: &[u8],
    ) -> bool {
        false
    }

    /// Bytes on one of Create's channels from the server, on the client. True when this
    /// subsystem owns `channel`.
    fn server_message(&self, _host: &mut dyn ClientHost, _channel: &str, _data: &[u8]) -> bool {
        false
    }

    /// Each tick of a client's level.
    fn client_tick(&self, _host: &mut dyn ClientHost) {}

    /// The model files (no block state names them) this subsystem's renderers draw through
    /// `RenderAssets::model`, baked on every resource load (`standalone_models`).
    fn standalone_models(&self, _models: &mut Vec<String>) {}

    /// The client left its world: drop what it kept.
    fn client_reset(&self) {}

    /// `Hooks::register_particle_providers`: the mod's particle types moved by code.
    fn particle_providers(&self, _providers: &mut dyn mod_api::ParticleProviders) {}

    /// `Hooks::register_special_model_renderer`: the special item models the mod draws.
    fn special_model_renderers(&self, _renderers: &mut dyn mod_api::SpecialModelRenderers) {}

    /// `Hooks::model_baking_completed`: resolve the standalone models drawn from now on.
    fn models_baked(&self, _host: &mut dyn ClientHost) {}

    /// `Hooks::entity_place`: a block is about to be placed.
    fn entity_place(&self, _host: &mut dyn ServerHost, _event: &mut EntityPlaceEvent) -> Flow {
        Flow::Continue
    }

    /// `Hooks::right_click_block`, on either side.
    fn right_click_block(&self, _host: HostSide, _event: &mut RightClickBlockEvent<'_>) -> Flow {
        Flow::Continue
    }

    /// `Hooks::right_click_item`, on either side.
    fn right_click_item(&self, _host: HostSide, _event: &mut RightClickItemEvent<'_>) -> Flow {
        Flow::Continue
    }

    /// `Hooks::left_click_block`.
    fn left_click_block(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut LeftClickBlockEvent,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::player_pick_item`: a server player picks an item from a block or an entity.
    fn player_pick_item(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut mod_api::PlayerPickItemEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::break_block`: a player breaks a block.
    fn break_block(&self, _host: &mut dyn ServerHost, _event: &mut BreakBlockEvent<'_>) -> Flow {
        Flow::Continue
    }

    /// `Hooks::block_drops`: a broken block's drops, before they are in the world.
    fn block_drops(
        &self,
        _level: &dyn mod_api::LevelHost,
        _event: &mut mod_api::BlockDropsEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::entity_interact`: a player right-clicks an entity. The first `Some` wins.
    fn entity_interact(
        &self,
        _host: HostSide,
        _event: &mut EntityInteractEvent<'_>,
    ) -> Option<InteractionResult> {
        None
    }

    /// Whether this subsystem's projectiles need `projectile_impact` (`Hooks::hears_projectile_impacts`).
    fn hears_projectile_impacts(&self) -> bool {
        false
    }

    /// `Hooks::projectile_impact`: a projectile hit something; cancel to take the hit.
    fn projectile_impact(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut mod_api::ProjectileImpactEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::attack_entity`.
    fn attack_entity(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut AttackEntityEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::entity_mount`: an entity mounts or dismounts.
    fn entity_mount(&self, _host: &mut dyn ServerHost, _event: &mut EntityMountEvent<'_>) -> Flow {
        Flow::Continue
    }

    /// `Hooks::player_tick_pre`, on either side.
    fn player_tick_pre(&self, _host: HostSide, _event: &mut mod_api::PlayerTickPreEvent) {}

    /// `Hooks::player_tick_post`, on either side.
    fn player_tick(&self, _host: HostSide, _event: &mut PlayerTickPostEvent) {}

    /// `Hooks::living_incoming_damage`: a living entity is about to be hurt; the first
    /// subsystem to answer `Flow::Cancel` spares it.
    fn living_incoming_damage(
        &self,
        _level: &mut dyn mod_api::LevelRandomHost,
        _event: &mut mod_api::LivingIncomingDamageEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::render_fog`: the world's fog on a client, before it is drawn.
    fn render_fog(&self, _host: &dyn ClientHost, _event: &mut mod_api::RenderFogEvent<'_>) {}

    /// `LivingKnockBackEvent`: a living entity is about to be pushed.
    fn living_knockback(
        &self,
        _level: &mut dyn mod_api::LevelRandomHost,
        _event: &mut mod_api::LivingKnockbackEvent<'_>,
    ) {
    }

    /// `Hooks::extract_block_screen_effect`: a first-person overlay (fire, water, a block) is
    /// about to be drawn; the first subsystem to answer `Flow::Cancel` leaves it out.
    fn screen_effect(
        &self,
        _host: &dyn ClientHost,
        _event: &mut mod_api::ExtractBlockScreenEffectEvent,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::register_commands`: push this subsystem's commands.
    fn register_commands(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut RegisterCommandsEvent<'_>,
    ) {
    }

    /// `Hooks::run_mod_command`: `Flow::Cancel` when the command was this subsystem's.
    fn run_mod_command(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::level_save`.
    fn level_save(&self, _host: &mut dyn ServerHost, _event: &mut LevelSaveEvent<'_>) {}

    /// `Hooks::input_key`, on the client.
    fn input_key(&self, _host: &mut dyn ClientHost, _event: &mut InputKeyEvent<'_>) {}

    /// `Hooks::camera_pose`, on the client: the camera's turn this frame.
    fn camera_pose(&self, _host: &dyn ClientHost, _event: &mut CameraPoseEvent<'_>) {}

    /// `Hooks::mouse_scrolling`, on the client: `Flow::Cancel` when the scroll was taken.
    fn mouse_scrolling(
        &self,
        _host: &mut dyn ClientHost,
        _event: &mut MouseScrollingEvent,
    ) -> Flow {
        Flow::Continue
    }

    /// After a key or mouse button was handled (`input_key`, `mouse_button_post`), with a
    /// client that may be changed: what the read-only screen hooks left (a screen to close, edits
    /// to send) goes now, in the same input as Create's, not at the next client tick.
    fn input_handled(&self, _host: &mut dyn ClientHost) {}

    /// `Hooks::interaction_key_mapping_triggered`, on the client: `Flow::Cancel` when the
    /// click was taken.
    fn interaction_key(
        &self,
        _host: &mut dyn ClientHost,
        _event: &mut InteractionKeyMappingTriggeredEvent,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::entity_join_level`: an entity joins a server level.
    fn entity_join_level(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut EntityJoinLevelEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::movement_input`, on the client: the local player's movement keys.
    fn movement_input(&self, _host: &dyn ClientHost, _event: &mut MovementInputEvent) {}
    /// `Hooks::key_mapping_pre`, on the client: `Flow::Cancel` keeps the key from the game.
    fn key_mapping_pre(
        &self,
        _host: &mut dyn ClientHost,
        _event: &mut KeyMappingPreEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::render_gui_post`: draw on the HUD.
    fn render_gui_post(&self, _host: &dyn ClientHost, _event: &mut RenderGuiPostEvent<'_>) {}

    /// `Hooks::data_pack_files`: pack files (`<kind>/<namespace>:<path>`, JSON) for this load.
    fn data_pack_files(&self, _files: &mut Vec<(String, Vec<u8>)>) {}

    /// `Hooks::client_information_updated`: a player's client settings (language, main hand).
    fn client_info(&self, _player: u128, _info: &mod_api::ClientInfo) {}

    /// `Hooks::living_entity_use_item_tick`, on the server: each tick of an item's use.
    fn use_item_tick(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut mod_api::LivingEntityUseItemTickEvent<'_>,
    ) {
    }

    /// `Hooks::living_entity_use_item_finish`, on the server: an item's use ran its time.
    fn use_item_finished(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut mod_api::LivingEntityUseItemFinishEvent<'_>,
    ) {
    }

    /// `Hooks::render_level_stage`: draw in the world at one stage of the frame.
    fn render_level_stage(
        &self,
        _host: &dyn ClientHost,
        _event: &mut mod_api::RenderLevelStageEvent<'_>,
    ) {
    }

    /// The wrench used on this subsystem's Create block `block` at `pos`, `clicked` the face at
    /// `point` (`IWrenchable.onWrenched`, or `onSneakWrenched` when `sneaking`), on the server:
    /// true when the block took it its own way, and the wrench then neither turns nor picks it
    /// up. Asked by the wrench (`items::wrench`) in `SUBSYSTEMS` order.
    #[allow(clippy::too_many_arguments)]
    fn wrench_used(
        &self,
        _lv: &mut ServerLevel<'_>,
        _player: PlayerRef,
        _pos: [i32; 3],
        _block: create_engine::Block,
        _clicked: create_engine::kinetics::Dir,
        _point: [f64; 3],
        _sneaking: bool,
    ) -> bool {
        false
    }

    /// Lines this subsystem adds to the goggles' tooltip for its Create block `block` at `pos`
    /// under the client's crosshair (`IHaveGoggleInformation.addToGoggleTooltip`): each line a
    /// run of `(text, rgb)`. Asked by the goggles' overlay (`interaction`).
    fn goggle_tooltip(
        &self,
        _host: &dyn ClientHost,
        _pos: [i32; 3],
        _block: create_engine::Block,
        _lines: &mut Vec<Vec<(String, u32)>>,
    ) {
    }

    /// `Hooks::player_container_open`: a player opened a menu.
    fn player_container_open(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut PlayerContainerOpenEvent<'_>,
    ) {
    }

    /// `Hooks::player_container_close`: a player's menu closed.
    fn player_container_close(
        &self,
        _host: &mut dyn ServerHost,
        _event: &mut PlayerContainerCloseEvent<'_>,
    ) {
    }
    /// `Hooks::menu_closed`: a menu a mod opened (`open_menu`) closed.
    fn menu_closed(&self, _host: &mut dyn ServerHost, _event: &mut MenuClosedEvent) {}

    /// `Hooks::screen_render_post`: draw over the open screen (the mod's own, `mod_screen`).
    fn screen_render_post(&self, _host: &dyn ClientHost, _event: &mut ScreenRenderPostEvent<'_>) {}

    /// `Hooks::screen_mouse_button_pressed_pre`: `Flow::Cancel` when the click was taken.
    fn screen_mouse_pressed(
        &self,
        _host: &dyn ClientHost,
        _event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::screen_mouse_button_released_pre`.
    fn screen_mouse_released(
        &self,
        _host: &dyn ClientHost,
        _event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::screen_mouse_scrolled_pre`.
    fn screen_mouse_scrolled(
        &self,
        _host: &dyn ClientHost,
        _event: &mut ScreenMouseScrolledPreEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::screen_key_pressed_pre`.
    fn screen_key_pressed(
        &self,
        _host: &dyn ClientHost,
        _event: &mut ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::screen_key_released_pre`.
    fn screen_key_released(
        &self,
        _host: &dyn ClientHost,
        _event: &mut mod_api::ScreenKeyReleasedPreEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::screen_char_typed_pre`.
    fn screen_char_typed(
        &self,
        _host: &dyn ClientHost,
        _event: &mut ScreenCharTypedPreEvent<'_>,
    ) -> Flow {
        Flow::Continue
    }

    /// `Hooks::screen_closing`: the open screen is being replaced or closed.
    fn screen_closing(&self, _host: &dyn ClientHost, _event: &mut ScreenClosingEvent<'_>) {}
}
