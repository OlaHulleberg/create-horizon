//! Create 6.0.11 for Horizon, as a native mod on the fork's mod API.
//!
//! Create's rules are the `create-engine` crate (`src/engine`): plain Rust with no game types,
//! unit-tested on their own. `glue` drives them through the mod API: each hook below is written
//! once and handed to every subsystem (`glue::SUBSYSTEMS`); per-block moments are Parts.

mod glue;

use glue::server::ServerLevel;
use glue::{SUBSYSTEMS, persist, states, world};
use mod_api::{
    AttackEntityEvent, BlockChangedEvent, BlockPart, BreakBlockEvent, CameraPoseEvent,
    ChunkLoadEvent, ChunkUnloadEvent, ClientHost, ClientLevelUnloadEvent, ClientTickPostEvent,
    EntityInteractEvent, EntityJoinLevelEvent, EntityMountEvent, EntityPlaceEvent, Flow, Hooks,
    HostSide, InputKeyEvent, InteractionKeyMappingTriggeredEvent, InteractionResult,
    KeyMappingPreEvent, LeftClickBlockEvent, LevelHost, LevelLoadEvent, LevelSaveEvent,
    LevelTickPostEvent, LevelTicksHost, MenuClosedEvent, MouseScrollingEvent, MovementInputEvent,
    NeighborChangedEvent, PlayerContainerCloseEvent, PlayerContainerOpenEvent, PlayerLoggedInEvent,
    PlayerTickPostEvent, RegisterCommandsEvent, Registrar, RegistrationContext, RenderGuiPostEvent,
    RightClickBlockEvent, RightClickItemEvent, RunModCommandEvent, ScreenCharTypedPreEvent,
    ScreenClosingEvent, ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent,
    ScreenMouseButtonReleasedPreEvent, ScreenMouseScrolledPreEvent, ScreenRenderPostEvent,
    ServerHost, StandaloneModelsEvent, TagsUpdatedEvent, WorldLifecycleEvent, WorldPhase,
};

#[derive(Default)]
struct Create;

#[mod_api::horizon_mod]
impl Hooks for Create {
    fn register(&self, r: &mut dyn Registrar, cx: &RegistrationContext) {
        states::register(r);
        for &block in create_engine::Block::ALL.iter() {
            r.block_part(states::key(block), CreateBlock);
        }
        glue::net::register(r, SUBSYSTEMS);
        for s in SUBSYSTEMS {
            s.register(r, cx);
        }
    }

    fn standalone_models(&self, _cx: &RegistrationContext, event: &mut StandaloneModelsEvent<'_>) {
        for s in SUBSYSTEMS {
            s.standalone_models(event.models);
        }
    }

    /// `AllBlocks.CONTROLLER_RAIL.color(..)`: the controller rail's middle in the redstone
    /// colour of its `power` (`RedStoneWireBlock.getColorForPower`).
    fn register_block_tint_sources(
        &self,
        _cx: &RegistrationContext,
        event: &mut mod_api::RegisterBlockTintSourcesEvent<'_>,
    ) {
        event.sources.register(
            vec![mod_api::TintLayer::Vanilla(mod_api::VanillaTint::Redstone)],
            vec!["create:controller_rail".to_owned()],
        );
    }

    fn world_lifecycle(&self, _host: &mut dyn ServerHost, event: &mut WorldLifecycleEvent<'_>) {
        match event.phase {
            WorldPhase::WorldCloseFinal
            | WorldPhase::WorldOpenFailed
            | WorldPhase::WorldCloseFailed => {
                for s in SUBSYSTEMS {
                    s.world_closed();
                }
                world::close();
                persist::clear();
                glue::net::clear();
                glue::forget_columns();
            }
            _ => {}
        }
    }

    /// A level opened, before any of its chunks: the first opens Create's world.
    fn level_load(&self, host: &mut dyn ServerHost, event: &mut LevelLoadEvent<'_>) {
        if !world::is_open() && world::open(&*host) {
            for s in SUBSYSTEMS {
                s.world_opened(host);
            }
        }
        world::level_index(event.dimension);
    }

    fn tags_updated(&self, host: HostSide, _event: &mut TagsUpdatedEvent) {
        let HostSide::Server(host) = host else {
            return;
        };
        for s in SUBSYSTEMS {
            s.tags_updated(host);
        }
    }

    fn chunk_load(&self, host: &mut dyn ServerHost, event: &mut ChunkLoadEvent<'_>) {
        glue::ensure_column(host, event.dimension, event.chunk);
    }

    fn chunk_unload(&self, _host: &mut dyn ServerHost, event: &mut ChunkUnloadEvent<'_>) {
        let Some(dim) = world::level_index(event.dimension) else {
            return;
        };
        glue::forget_column(dim, event.chunk);
        for s in SUBSYSTEMS {
            s.chunk_unload(dim, event.chunk);
        }
    }

    /// Hot: an entity on the ground over one of Create's blocks, each tick it moves.
    fn block_step_on(&self, event: &mut mod_api::BlockStepOnEvent<'_>) {
        glue::logistics::block_step_on(event);
    }

    /// Hot: every committed block change on the server. Two lookups, then return, for any
    /// change that is not Create's.
    fn block_changed(&self, _level: &dyn LevelHost, event: &mut BlockChangedEvent) {
        let (old, new) = (states::id_or_none(event.old), states::id_or_none(event.new));
        if old == 0 && new == 0 {
            return;
        }
        let Some(dim) = world::level_index(event.dimension.key().id()) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        for s in SUBSYSTEMS {
            s.block_changed(dim, pos, old, new);
        }
    }

    fn fluid_tick(&self, level: &dyn LevelHost, event: &mut mod_api::FluidTickEvent<'_>) -> Flow {
        for s in SUBSYSTEMS {
            s.fluid_tick(level, event);
        }
        if event.cancelled {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    fn level_tick_post(&self, host: HostSide, event: &mut LevelTickPostEvent) {
        let HostSide::Server(host) = host else {
            return;
        };
        let Some(dimension) = event.level.dimension().id(&*host) else {
            return;
        };
        glue::net::flush(host);
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        for s in SUBSYSTEMS {
            s.level_tick(&mut lv);
        }
        persist::flush(&mut lv, SUBSYSTEMS);
    }

    fn player_logged_in(&self, host: &mut dyn ServerHost, event: &mut PlayerLoggedInEvent<'_>) {
        if !world::is_open() {
            return;
        }
        let Some(player) = host.player(event.player) else {
            return;
        };
        for s in SUBSYSTEMS {
            s.player_joined(host, player);
        }
    }

    fn client_tick_post(&self, host: &mut dyn ClientHost, _event: &mut ClientTickPostEvent) {
        for s in SUBSYSTEMS {
            s.client_tick(host);
        }
    }

    fn model_baking_completed(
        &self,
        host: &mut dyn ClientHost,
        _event: &mut mod_api::ModelBakingCompletedEvent,
    ) {
        for s in SUBSYSTEMS {
            s.models_baked(host);
        }
    }

    fn entity_place(&self, host: &mut dyn ServerHost, event: &mut EntityPlaceEvent) -> Flow {
        until_cancel(|s| s.entity_place(host, event))
    }

    fn register_particle_providers(
        &self,
        _context: &RegistrationContext,
        event: &mut mod_api::RegisterParticleProvidersEvent<'_>,
    ) {
        for s in SUBSYSTEMS {
            s.particle_providers(&mut *event.providers);
        }
    }

    fn register_render_pipelines(
        &self,
        _context: &RegistrationContext,
        event: &mut mod_api::RegisterRenderPipelinesEvent<'_>,
    ) {
        glue::pipelines::register(event);
    }

    fn register_item_models(
        &self,
        _context: &RegistrationContext,
        event: &mut mod_api::RegisterItemModelsEvent<'_>,
    ) {
        event.models.push(glue::equipment::GRIP_POSE.into());
    }

    fn item_model_update(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ItemModelUpdateEvent<'_>,
    ) {
        glue::equipment::grip_pose(host, event);
    }

    fn register_special_model_renderer(
        &self,
        _context: &RegistrationContext,
        event: &mut mod_api::RegisterSpecialModelRendererEvent<'_>,
    ) {
        for s in SUBSYSTEMS {
            s.special_model_renderers(&mut *event.renderers);
        }
    }

    fn right_click_block(&self, mut host: HostSide, event: &mut RightClickBlockEvent<'_>) -> Flow {
        until_cancel(|s| s.right_click_block(reborrow(&mut host), event))
    }

    fn right_click_item(&self, mut host: HostSide, event: &mut RightClickItemEvent<'_>) -> Flow {
        until_cancel(|s| s.right_click_item(reborrow(&mut host), event))
    }

    fn left_click_block(&self, host: &mut dyn ServerHost, event: &mut LeftClickBlockEvent) -> Flow {
        until_cancel(|s| s.left_click_block(host, event))
    }

    fn break_block(&self, host: &mut dyn ServerHost, event: &mut BreakBlockEvent<'_>) -> Flow {
        until_cancel(|s| s.break_block(host, event))
    }

    fn block_drops(&self, level: &dyn LevelHost, event: &mut mod_api::BlockDropsEvent<'_>) -> Flow {
        until_cancel(|s| s.block_drops(level, event))
    }

    fn entity_interact(
        &self,
        mut host: HostSide,
        event: &mut EntityInteractEvent<'_>,
    ) -> Option<InteractionResult> {
        SUBSYSTEMS
            .iter()
            .find_map(|s| s.entity_interact(reborrow(&mut host), event))
    }

    fn attack_entity(&self, host: &mut dyn ServerHost, event: &mut AttackEntityEvent<'_>) -> Flow {
        until_cancel(|s| s.attack_entity(host, event))
    }

    fn hears_projectile_impacts(
        &self,
        _host: &dyn mod_api::Host,
        event: &mut mod_api::HearsProjectileImpactsEvent,
    ) {
        event.hears |= SUBSYSTEMS.iter().any(|s| s.hears_projectile_impacts());
    }

    fn projectile_impact(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::ProjectileImpactEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.projectile_impact(host, event))
    }

    fn entity_mount(&self, host: &mut dyn ServerHost, event: &mut EntityMountEvent<'_>) -> Flow {
        until_cancel(|s| s.entity_mount(host, event))
    }

    fn entity_join_level(
        &self,
        host: &mut dyn ServerHost,
        event: &mut EntityJoinLevelEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.entity_join_level(host, event))
    }

    fn key_mapping_pre(
        &self,
        host: &mut dyn ClientHost,
        event: &mut KeyMappingPreEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.key_mapping_pre(host, event))
    }

    fn player_tick_pre(&self, mut host: HostSide, event: &mut mod_api::PlayerTickPreEvent) {
        for s in SUBSYSTEMS {
            s.player_tick_pre(reborrow(&mut host), event);
        }
    }

    fn player_tick_post(&self, mut host: HostSide, event: &mut PlayerTickPostEvent) {
        for s in SUBSYSTEMS {
            s.player_tick(reborrow(&mut host), event);
        }
    }

    fn living_incoming_damage(
        &self,
        level: &mut dyn mod_api::LevelRandomHost,
        event: &mut mod_api::LivingIncomingDamageEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.living_incoming_damage(level, event))
    }

    fn living_knockback(
        &self,
        level: &mut dyn mod_api::LevelRandomHost,
        event: &mut mod_api::LivingKnockbackEvent<'_>,
    ) -> Flow {
        for s in SUBSYSTEMS {
            s.living_knockback(level, event);
        }
        Flow::Continue
    }

    fn render_fog(&self, host: &dyn ClientHost, event: &mut mod_api::RenderFogEvent<'_>) {
        for s in SUBSYSTEMS {
            s.render_fog(host, event);
        }
    }

    fn extract_block_screen_effect(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ExtractBlockScreenEffectEvent,
    ) -> Flow {
        until_cancel(|s| s.screen_effect(host, event))
    }

    fn register_commands(&self, host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        for s in SUBSYSTEMS {
            s.register_commands(host, event);
        }
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.run_mod_command(host, event))
    }

    fn level_save(&self, host: &mut dyn ServerHost, event: &mut LevelSaveEvent<'_>) {
        for s in SUBSYSTEMS {
            s.level_save(host, event);
        }
    }

    fn input_key(&self, host: &mut dyn ClientHost, event: &mut InputKeyEvent<'_>) {
        for s in SUBSYSTEMS {
            s.input_key(host, event);
        }
        for s in SUBSYSTEMS {
            s.input_handled(host);
        }
    }

    fn mouse_button_post(
        &self,
        host: &mut dyn ClientHost,
        event: &mut mod_api::MouseButtonPostEvent,
    ) {
        glue::equipment::grip_swing(host, event);
        for s in SUBSYSTEMS {
            s.input_handled(host);
        }
    }

    fn camera_pose(&self, host: &dyn ClientHost, event: &mut CameraPoseEvent<'_>) {
        for s in SUBSYSTEMS {
            s.camera_pose(host, event);
        }
    }

    fn mouse_scrolling(&self, host: &mut dyn ClientHost, event: &mut MouseScrollingEvent) -> Flow {
        until_cancel(|s| s.mouse_scrolling(host, event))
    }

    fn interaction_key_mapping_triggered(
        &self,
        host: &mut dyn ClientHost,
        event: &mut InteractionKeyMappingTriggeredEvent,
    ) -> Flow {
        until_cancel(|s| s.interaction_key(host, event))
    }

    fn movement_input(&self, host: &dyn ClientHost, event: &mut MovementInputEvent) {
        for s in SUBSYSTEMS {
            s.movement_input(host, event);
        }
    }

    fn living_entity_use_item_finish(
        &self,
        host: HostSide,
        event: &mut mod_api::LivingEntityUseItemFinishEvent<'_>,
    ) {
        if let HostSide::Server(host) = host {
            for s in SUBSYSTEMS {
                s.use_item_finished(&mut *host, event);
            }
        }
    }

    fn render_level_stage(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::RenderLevelStageEvent<'_>,
    ) {
        for s in SUBSYSTEMS {
            s.render_level_stage(host, event);
        }
    }

    fn render_gui_post(&self, host: &dyn ClientHost, event: &mut RenderGuiPostEvent<'_>) {
        for s in SUBSYSTEMS {
            s.render_gui_post(host, event);
        }
    }

    fn player_container_open(
        &self,
        host: &mut dyn ServerHost,
        event: &mut PlayerContainerOpenEvent<'_>,
    ) {
        for s in SUBSYSTEMS {
            s.player_container_open(host, event);
        }
    }

    fn player_container_close(
        &self,
        host: &mut dyn ServerHost,
        event: &mut PlayerContainerCloseEvent<'_>,
    ) {
        for s in SUBSYSTEMS {
            s.player_container_close(host, event);
        }
    }

    fn menu_closed(&self, host: &mut dyn ServerHost, event: &mut MenuClosedEvent) {
        for s in SUBSYSTEMS {
            s.menu_closed(host, event);
        }
    }

    fn add_server_reload_listeners(
        &self,
        _host: &dyn mod_api::Host,
        event: &mut mod_api::AddServerReloadListenersEvent<'_>,
    ) {
        glue::items::add_reload_listeners(event.listeners);
    }

    fn register_range_select_item_model_property(
        &self,
        _context: &RegistrationContext,
        event: &mut mod_api::RegisterRangeSelectItemModelPropertyEvent<'_>,
    ) {
        glue::items::spin_properties(event.properties);
    }

    fn item_model_range_property(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ItemModelRangePropertyEvent<'_>,
    ) {
        if let Some(frame) = glue::items::spin(host, event.property) {
            event.value = frame;
        }
    }

    fn item_tooltip(&self, host: &dyn ClientHost, event: &mut mod_api::ItemTooltipEvent<'_>) {
        glue::items::item_tooltip(host, event);
    }

    fn screen_render_post(&self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        for s in SUBSYSTEMS {
            s.screen_render_post(host, event);
        }
    }

    fn screen_mouse_button_pressed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.screen_mouse_pressed(host, event))
    }

    fn screen_mouse_button_released_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.screen_mouse_released(host, event))
    }

    fn screen_mouse_scrolled_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseScrolledPreEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.screen_mouse_scrolled(host, event))
    }

    fn screen_key_pressed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.screen_key_pressed(host, event))
    }

    fn screen_key_released_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenKeyReleasedPreEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.screen_key_released(host, event))
    }

    fn screen_char_typed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenCharTypedPreEvent<'_>,
    ) -> Flow {
        until_cancel(|s| s.screen_char_typed(host, event))
    }

    fn screen_closing(&self, host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
        for s in SUBSYSTEMS {
            s.screen_closing(host, event);
        }
    }

    fn client_level_unload(
        &self,
        host: &mut dyn ClientHost,
        _event: &mut ClientLevelUnloadEvent<'_>,
    ) {
        glue::client_reset(host);
    }
}

/// Asks each subsystem in order until one cancels.
fn until_cancel(mut f: impl FnMut(&dyn glue::subsystem::Subsystem) -> Flow) -> Flow {
    for s in SUBSYSTEMS {
        if let Flow::Cancel = f(*s) {
            return Flow::Cancel;
        }
    }
    Flow::Continue
}

/// The same host again, for the next subsystem.
fn reborrow<'a>(host: &'a mut HostSide<'_>) -> HostSide<'a> {
    match host {
        HostSide::Server(h) => HostSide::Server(&mut **h),
        HostSide::Client(h) => HostSide::Client(&mut **h),
    }
}

/// Every Create block's part: the engine's per-block moments. Subsystems attach their own
/// parts to their own blocks for the rest.
struct CreateBlock;

#[mod_api::horizon_part]
impl BlockPart for CreateBlock {
    /// A neighbour of a Create block changed (`neighborChanged`). Runs inside the write: the
    /// subsystems note it and act in the level tick.
    fn neighbor_changed(&self, _level: &mut dyn LevelTicksHost, event: &mut NeighborChangedEvent) {
        let Some(state) = states::id(event.state) else {
            return;
        };
        let Some(dim) = world::level_index(event.dimension.key().id()) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        for s in SUBSYSTEMS {
            s.neighbor_changed(dim, pos, state);
        }
    }
}
