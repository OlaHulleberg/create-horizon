# Create 6.0.11 coverage ledger for Horizon's mod API (#7386 stage 6)

**Create** is the NeoForge tech mod of rotational power: shafts and gears, belts, funnels, presses, fans, pipes, moving contraptions, trains and copycat blocks. Its code is MIT; its assets are ARR.

**Where this comes from.** This ledger is not a guess at what Create would need. It comes from a working port of Create 6.0.11 to Horizon, a horizon-mixins native bundle (`[mod] id = "create"`):
- **Mod:** https://github.com/OlaHulleberg/create-horizon. The glue is `mixins/create/*.rs` (~39k lines; 32 `[[module]]`s, 30 of them in `game.rs`, plus 42 HEAD `[[injection]]`s in `horizon-mixin.toml`). The pure rules are in `mixins/create/engine`.
- **Horizon hooks it needs:** https://github.com/OlaHulleberg/horizon/tree/feat/create-hooks. That branch is 53 commits on `9d979ca` (2026-10-31) and adds `NATIVE_HOOKS_API` 41–67.
- **Upstream compared against:** `horizon-rust/horizon` `origin/main` at `68ff46998` (2026-11-02 21:24, API 359). PR and issue states were read on 2026-11-02 with `gh` (read-only).

**How to read a row.**
- **Need:** the need in plain words.
- **Create:** the Java class or method, or the NeoForge event, that Create relies on.
- **Port today:** what the port uses now.
  - A hook or helper with its API level. "ours N" means `feat/create-hooks` numbering, which **collides** with main's numbers (our 41 is not main's 41).
  - Levels ≤ 40 are `9d979ca`'s and keep the same number on main.
  - "module" means the glue reaches into `Game` internals directly; see §3.
- **Upstream:** the status on Horizon, in one of five categories:
  - **covered**: merged on main, with the name and main's API level;
  - **review**: an open PR, with its number and author;
  - **paused**: a #7386 line on hold, named;
  - **design**: an issue awaiting a decision, with the issue and its owner;
  - **none**: nothing upstream.
  - "(partial)" means the row needs more than the cited item gives.
- **Use:** two counts.
  - `g N` is glue call sites in the port, counted by grep over `mixins/create/*.rs`.
  - `j N` is the textual grep hits for the method or class in Create's `src/main/java`. These are not call-graph counts, but they serve as #7387-style "mod calls" for priority.
- **Numbers:** `#N` is a horizon-rust/horizon issue or PR, and `fork#N` is one on arismoko/horizon.

---

## 1. Foundation (shared by every subsystem)

| # | Need | Create | Port today | Upstream | Use |
|---|---|---|---|---|---|
| F1 | Read a block state anywhere, fast | `Level.getBlockState` | module: `game.world.block` | **review** #7559 (levzzz5154) `LevelRef::block_state`/`block`/`block_property`. Returns a `String`; Create's hot loops need a state-id read | g 272, j 241 |
| F2 | Game and day time | `getGameTime`, `getDayTime` | module: `game.world.game_time`/`day_time` | **review** #7559 `game_time`, `day_time` (main has `native_day_time`, API 97) | g 87, j 23 |
| F3 | Block registry: name ↔ state id, default state, property values | `Block.defaultBlockState`, `BlockState.setValue` | module: `game.pack.assets.registry` | **covered (partial)** `native_block_property` (API 20), `native_state_with_property` (76), `native_state_block` (164). `block_state_default` is an open fork#29 line | g 151 |
| F4 | Block facts: solidity, behaviours, tags | `BlockStateBase.*`, `BlockBehaviour` | module: `game.res.behaviours`/`facts` | **covered (partial)** for tags: `native_block_in_tag` (28), `native_server_block_in_tag` (63). The `block_state_info`/`block_context_info` lines are open (fork#29) | g 55 |
| F5 | Set a block or a property, with flags | `Level.setBlock` | `native_set_block` (24), `native_set_block_property(_with)` (18/33) | **covered** same names (API 24, 18, 33) | g 129 |
| F6 | Break a block with its drops | `Level.destroyBlock(pos, true)` | `/execute … setblock … destroy` via `native_run_command`; module `with_level(… destroy_block)` | **none**: #5552 item 1 (unclaimed); fork#29 "block writes" `destroy_block` (open) | g 9 |
| F7 | Spawn item entities with motion | `Block.popResource`, `ItemEntity` | `/execute … summon minecraft:item` via `native_run_command` | **covered** `native_summon` (API 59). The handle form, LevelRef `spawn`, is an open line | g 22 |
| F8 | Play a sound at a block | `Level.playSound` | `/playsound` via `native_run_command` (17); client `game.audio.play` (19) | **covered** `native_play_sound` (25, 61), `native_play_local_sound` (83). **review** #7676 (levzzz5154) `LevelRef::play_sound` | g 36, j 173 |
| F9 | Particles | `Level.addParticle` | mostly not ported; the deployer calls the private `transform_particle` | **covered** `native_send_particles` (11, 103). **review** #7676 | j 95 |
| F10 | Block entity with typed state, ticked | `SmartBlockEntity`, `tick()`, `lazyTick()` | the mod's own arena keyed by position, ticked from `level_tick_post`; `native_block_entity` only for slot machines; `native_with_mod_block_entity` | **covered** `native_block_entity`/`_tick` (15, 17, 129), attachments `Holder::BlockEntity` (#6216). **review** fork#38 (arismoko) read-only `BlockEntityRef` | g 28, j 226 |
| F11 | Block entity state sent to clients | `SmartBlockEntity.sendData`, `notifyUpdate` | own mod messages: `native_net_send_to_player` + `net_from_server` | **covered** `native_block_entity_update_tag` (45), `native_block_entity_sync` (52), attachments `sync: true` (#7224) | g 33, j 395 |
| F12 | Mod messages client ↔ server | `CatnipPacket`, `BlockEntityConfigurationPacket` | `native_net_send_to_player`/`_to_server`, `net_from_client`/`_server` (40) | **covered** same names (API 40) | g 41 |
| F13 | Right-click or use on a block | `useItemOn`, `useWithoutItem`, `RightClickBlock` | HEAD `right_click_block` (21), `use_item_on`, `use_item` (13) | **covered** (21, 13). A scoped per-block form waits on #7386 parts | j 95 |
| F14 | Placement state | `getStateForPlacement` | HEAD `place_block` (14), then `native_set_block_property` | **covered** `native_state_for_placement` (76), a better fit | j 167 |
| F15 | Own block placed or removed | `onPlace`, `onRemove` | HEAD `block_changed` (7), a hot broadcast with a range test | **covered** `native_block_changed` (7) as a broadcast. A scoped `onPlace`/`onRemove`: **none** (#7386 scope) | j 152 |
| F16 | Shape updates, survival, neighbours | `updateShape`, `canSurvive`, `neighborChanged` | HEAD `neighbor_changed` (6); `block_changed` | **covered** `native_update_shape` (32), `native_can_survive` (70), `native_neighbor_changed` (6) | j 177 |
| F17 | Commands | `RegisterCommandsEvent` | HEAD `register_commands`, `perform_command` (8) | **covered** (8) | |
| F18 | Acting player, their items, feedback | `Player.getItemInHand`, `displayClientMessage` | `native_acting_player`, `native_held`, `native_give_items`, `native_take_items`, `native_feedback`, `native_action_bar` (9, 21) | **covered** same names. Handle form: **review** #7563 (levzzz5154) LivingRef equipment; fork#30 (arismoko, draft) PlayerRef | g 130 |
| F19 | Player state: creative, hands, inventory, look | `isCreative`, `getInventory`, `getYRot` | module: `game.creative`/`inventory`/`selected`/`hand`/`offhand`/`armor`/`yaw`/`pitch`/`may_build` | **covered** `rotation` (#7454). **review** fork#30 (`is_creative`, `abilities`), #7563 (hands). **paused** PlayerRef "inventory and containers" | g 230 |

## 2. Per subsystem

### Kinetics (shafts, gears, motors, water wheels, fans, turntables)

| # | Need | Create | Port today | Upstream | Use |
|---|---|---|---|---|---|
| K1 | Re-solve a rotation network when blocks change | `RotationPropagator`, `KineticNetwork` | `engine/kinetics` from `block_changed` + `level_tick_post` (23) | **covered**: hooks only (7, 23) | |
| K2 | Turning shafts, gears and cogs drawn | `KineticBlockEntityRenderer`, Flywheel `Instancer` | ours 42/45/48/50 parts, see RN1 | **design** #6302 (levzzz5154, B/C open) | see RN1 |
| K3 | Speed and stress shown on goggles | `IHaveGoggleInformation` | HEAD `hud` + `native_text`, `native_text_width`, `native_fill` | **covered** `native_hud` (base), `native_gui_layer_pre/post` (139) | g 27 |
| K4 | Scroll value on a block (creative motor speed) | `ScrollValueBehaviour` | HEAD `wheel` (base) | **covered** `native_wheel`, `native_mouse_scrolling` (122) | |
| K5 | Wrench rotates or picks up blocks | `IWrenchable.onWrenched` | HEAD `use_item_on` (13) | **covered** (13) | j 98 |
| K6 | Water wheel reads the water's flow | `WaterWheelBlockEntity` → `FluidState.getFlow` | `native_fluid_flow` (ours 44) | **none**: the fork#29 LevelRef "fluids" line (`fluid_flow`) is unclaimed; its pause status is unclear | g 1 |
| K7 | Fan and turntable move, turn and catch entities | `EncasedFanBlockEntity`/`AirCurrent`, `TurntableBlockEntity` | `native_entities_in`, `native_set_delta_movement`, `native_set_yaw`, `native_reset_fall_distance` (ours 60) | **review** #7560 (levzzz5154) `velocity`/`set_velocity`, #7690 (levzzz5154, draft) `set_rotation`/`push`, fork#32 (arismoko, draft) `entities_in`. `reset_fall_distance` is **none** as far as seen (unverified) | g 15, j 89 |
| K8 | Entity standing in or on a turntable or seat | `entityInside` | `entity_inside` property (17 blocks) + hook (ours 61) | **covered** `native_block_entity_inside` (73, #5536) | j 8 |
| K9 | Cuckoo clock and nozzle explosions | `Level.explode` | not ported | **covered** `native_explode` (66) | j 3 |

### Processing (press, millstone, crusher, mixer, basin, fan processing, deployer, burner)

| # | Need | Create | Port today | Upstream | Use |
|---|---|---|---|---|---|
| P1 | Recipes of Create's own types | `ProcessingRecipe`, `RecipeManager` | `native_recipes_of_type`, `native_recipe_ingredient`, `native_recipe_result` (36) | **covered** (36). Our fix `b133ffc29` (empty in tick hooks) is not checked on main | g 19 |
| P2 | Items on a depot or belt under a press or fan | `DepotBehaviour`, `FanProcessing` | module `items_in` over `game.dropped` (ours 52 also adds `native_items_in`) | **covered** `native_item_entities`/`native_set_item_entity` (42). **review** #7560 (motion), fork#32 | g 11 |
| P3 | Deployer acting as a player (use, punch, place) | `DeployerFakePlayer`, `DeployerHandler` | module `create_use` in `game/dispenser.rs` + `native_use_item_at` (ours 54) | **none**: #5556 fake player (JuanLuci, unclaimed). Related: fork#30 PlayerRef (draft) | g 8, j 95 |
| P4 | Millstone takes only millable items | `IItemHandler.isItemValid` | none (it takes any item through a hopper) | **paused** (capability parts, #7386) | |
| P5 | Basin, depot and millstone slots for hoppers and comparators | `Capabilities.ItemHandler.BLOCK` | HEAD `block_entity_container` (16) | **covered** (16) for slot-backed blocks. Capability form **paused** | j 44 regs |
| P6 | Blaze burner fed, captured from a spawner | `BlazeBurnerBlockItem`, `BaseSpawner` | `interact_entity` (29); spawner capture not ported | **covered** `native_interact_entity` (29). Spawner potentials: **none** | |

### Logistics (belts, funnels, chutes, arms, packages, stock)

| # | Need | Create | Port today | Upstream | Use |
|---|---|---|---|---|---|
| L1 | An item landing on a belt or depot is taken | `Block.updateEntityAfterFallOn` | HEAD `update_entity_after_fall_on` (ours 49) | **none**. `native_block_fall_on` (73) is damage only. Needs a scoped hook (#7386 stage 1 bars it until parts) | j 26 |
| L2 | Pull items from any container through a face | `InvManipulationBehaviour`, `IItemHandler.extractItem` | module `create_extract` in `game/machines.rs` (ours 53 also adds `native_extract_item`) | **covered** `native_container_slots`, `native_extract_item`, `native_move_items` (48). Capability form **paused** | g 14, j 307 |
| L3 | Insert with room and per-item checks | `IItemHandler.insertItem` | `create_extract::insert_room`, `native_insert_item` (20) | **paused** (capability parts) | g 2 |
| L4 | Told when a hopper changes a depot's stack | `ItemStackHandler.onContentsChanged` | HEAD `contents_changed` + `ModContainer::only_when_empty` (ours 46) | **paused** (capability parts) | |
| L5 | Vanilla hoppers reach a Create inventory with no slots (creative crate) | `Capabilities.ItemHandler.BLOCK` | none | **paused** (capability parts) | j 44 regs |
| L6 | Items gliding on belts drawn | `BeltRenderer`/`BeltInventory` | `native_part_item_model`, `native_part_glide` (ours 45) | **design** #6302 step 3 (scrolling). No instanced item meshes; `native_draw_item` (91) is immediate only | g 25 |
| L7 | Package address and contents, filters, schedules on a stack | `DataComponents`, `CustomData` | module: stack NBT in `package_body.rs`/`items_body.rs` | **covered (partial)** `native_component_types` + NBT component helpers (65). ItemStackRef `component`/`set_component` is an open line (fork#29) | j 592 |
| L8 | Postbox shuts when its last menu closes | `AbstractContainerMenu.removed` | HEAD `menu_closed` (10) | **covered** (10), `native_mod_container_viewers` (124) | |
| L9 | Stock networks across restarts | `SavedData` | `native_saved_data` (ours 58) | **covered** attachments `Holder::Level` (#6216) | g 3 |

### Fluids (pipes, pumps, tanks, spouts, drains, hose pulleys, mod fluids)

| # | Need | Create | Port today | Upstream | Use |
|---|---|---|---|---|---|
| FL1 | Honey and chocolate as real fluids (source, flowing, bucket) | `FluidType`, `BaseFlowingFluid` | 5 engine commits on `feat/create-hooks` (content fluids) | **design** #6301 (levzzz5154's agent, A/B/C open), #5553 (JuanLuci) | 2 fluids |
| FL2 | Tanks and pipes exchange fluid with other blocks | `Capabilities.FluidHandler.BLOCK` | the engine's own network; no interop | **covered (partial)** `native_block_entity_fluids`, `native_fluid_tanks/insert/extract` (54), `native_move_fluids` (55). Capability form **paused** | j 44 regs |
| FL3 | Buckets and bottles fill or drain on spouts, drains and tanks | `Capabilities.FluidHandler.ITEM` | held, give and take helpers with item checks | **covered** `native_item_fluid_tank`, `native_item_fluid(_fill/_drain)` (147) | j 11 regs |
| FL4 | Read a world fluid (open-ended pipe, hose pulley) | `Level.getFluidState` | module: block reads; `waterlogged` property | **none**: the fork#29 `fluid_state` line is taken by levzzz5154 but has no PR | |
| FL5 | Fluid drawn in tanks and pipes | `FluidRenderer` | hidden `part_fluid_*` blocks + `native_part_model_tinted` (ours 50) | **design** #6302 + #6301 (rendering step) | g 1 |
| FL6 | Fluid ticks after a pipe places a source | `scheduleTick` | `native_set_block` | **covered** `native_schedule_fluid_tick` (32) | |

### Equipment and items (goggles, wrench, extendo grip, backtank, diving gear, wands, potato cannon)

| # | Need | Create | Port today | Upstream | Use |
|---|---|---|---|---|---|
| E1 | Tool wear and reach on break, hit and place | `ExtendoGripItem` | HEAD `destroy_block` (14), `hurt_enemy` (39), `place_block` (14), `player_tick` (31); module `game.player`, `damage_held` | **covered** for the hooks. Reach attribute: **none** (fork#29 LivingRef "attributes", open) | |
| E2 | Backtank air, diving helmet breathing | `BacktankUtil`, `LivingBreatheEvent` | `player_tick` + module (`equipment_body.rs`) | **covered** `native_living_breathe` (144), `native_component_types` (65) | |
| E3 | Wands and worldshaper aim at a block | `RayTraceHelper`, `Player.pick` | module `game.pick()`, `game.aim()` | **covered (partial)** `native_raycast`/`native_view_hit` (116, client). Server-side "targeted block" is #5552 item 3 (unclaimed) | g 23 |
| E4 | Item lit like a block in its model | `BakedModel.isGui3d` | `native_item_uses_block_light` (ours 47) | **none** | g 4, j 15 |
| E5 | Item cooldowns (belt connector) | `ItemCooldowns.addCooldown` | not ported | **none**: the PlayerRef "cooldowns" line is open. `native_player_item_cooldown` (262) only observes | j 18 |
| E6 | Advancements | `AllAdvancements`, `CriteriaTriggers` | not ported | **covered** `native_trigger_criterion` (69) | j 240 |
| E7 | Potato cannon projectiles | `PotatoProjectileEntity`, `ProjectileImpactEvent` | rules only (`engine/potato.rs`) | **design** #5212 (reservedbytes, assigned: mod entity types, throwables first). `native_projectile_impact` (136) is **covered** | |
| E8 | Clipboard text editing screen | `ClipboardScreen` (`MultiLineEditBox`) | not ported | **none** found. Unverified: whether `native_mod_screen` (111) can host a text box | |
| E9 | Toolbox and schematic table menus | `MenuType`, `Screen` | HEAD `screen` (19), `native_open_menu` (10) | **covered** (19, 10, 85, 309) | |

### Redstone (links, latches, analog lever, nixie tubes, display links, controllers)

| # | Need | Create | Port today | Upstream | Use |
|---|---|---|---|---|---|
| R1 | Own blocks give a signal | `getSignal`, `isSignalSource` | HEAD `block_signal`, `is_signal_source` (38) | **covered** (38) | j 46 |
| R2 | Tell neighbours of a signal change | `updateNeighborsAt` | `native_update_signal_neighbors` (38) | **covered** (38) | g 15 |
| R3 | Read redstone input | `hasNeighborSignal`, `getSignal(pos, dir)` | module `game.with_level(… has_neighbor_signal / best_neighbor_signal / signal)` | **covered (partial)** `native_neighbor_signal` (96, best of 6) for 12 sites; a directional read (6 sites): **none** | g 18 |
| R4 | Comparator output | `hasAnalogOutputSignal`, `getAnalogOutputSignal` | `analog_output` property (57 blocks) + HEAD `analog_output_signal`, `native_update_neighbour_for_output_signal` (ours 56) | **covered** `native_has_analog_output`, `native_analog_output`, `native_update_output_neighbors` (64, #5520) | j 43 |
| R5 | Latch and diode ticks of their own | `tick` (scheduled) | HEAD `tick_block` (34) | **covered** (34) | |
| R6 | Linked controller takes the movement keys | `LinkedControllerClientHandler`, `InputEvent.Key` | HEAD `input_key` with `consumed` + `native_is_key_down` (ours 57); module `game.keys` | **covered (partial)** `native_input_key` (121) is listen-only. Consuming a key: **none**. `is_key_down`: **none** | g 24 |
| R7 | Text drawn on nixie tubes and display boards | `NixieTubeRenderer`, `FlapDisplayRenderer` | `native_text_set`/`_clear` (ours 55) | **none**: #5552 item 4 (unclaimed), #5548 | g 4 |
| R8 | Controller rail powers carts | `ControllerRailBlock.onMinecartPass` | `rail` property (2 blocks) + HEAD `minecart_pass` (ours 62) | **none**: no line or issue; needs a scoped hook | j 6 |

### Contraptions and minecarts (bearings, pistons, pulleys, gantries, cart assemblers, seats, actors)

| # | Need | Create | Port today | Upstream | Use |
|---|---|---|---|---|---|
| C1 | Moving block structures as one entity | `AbstractContraptionEntity`, `Contraption`, `ContraptionCollider` | engine + `native_part_model_compound`/`_free` (ours 51) | **design** #5313 (sirrayi) and #6302 step 6 | g 16 |
| C2 | Contraption pushes and moves entities | `ContraptionCollider` → `setDeltaMovement` | `native_set_delta_movement` (ours 60) | **review** #7560 (levzzz5154) | g 7 |
| C3 | Spawn, find, move and remove minecarts | `MinecartContraptionItem`, `CartAssemblerBlockEntity` | `native_minecart(s)` (ours 62), `native_spawn_minecart`/`_remove_minecart` (ours 65) | **covered (partial)** `native_summon` (59), `native_vehicle_create/_damage/_destroy` (336–338), `native_vehicle_moved` (178). **review** #7690 (move). Removal is levzzz5154's claim (no PR seen) | g 8 |
| C4 | Furnace cart fuel, chest cart contents | `MinecartFurnace`, `MountedStorageManager` | not ported (fuel); contraption storage via module (`contraption_body.rs`) | **paused** (capability parts) | |
| C5 | Clicks on a moving contraption reach the mod | `InputEvent.InteractionKeyMappingTriggered` | HEAD `interaction_key_mapping_triggered` (ours 66) | **covered** `native_interaction_key` (138, #6289) | j 4 subs |
| C6 | Seats: spawn, move, remove, put on and take off riders | `SeatEntity`, `startRiding`, `stopRiding`, `getPassengers` | `native_spawn_seat`/`_move_seat`/`_remove_seat`, `native_start_riding`/`_stop_riding`, `native_passengers` (ours 63) on an `item_display` stand-in | **none**: fork#29 EntityRef and LivingRef "passengers and vehicles" are unclaimed; #4968 (displays as vehicles, ludovicthenot assigned); seat entity type #5212 | g 30, j 53 |
| C7 | Get on or off a vehicle, cancellable | `EntityMountEvent` | HEAD `entity_mount` (ours 63) | **covered** `native_entity_mount` (137, #6281). Not told for a removed vehicle or save-loaded riders (asked on #6281) | j 2 subs |
| C8 | A seat back from a save with its riders | `EntityJoinLevelEvent` (loaded from disk) | HEAD `seat_loaded` (ours 67) | **none**: `native_entity_join_level` (47) has `loaded_from_disk` but skips displays. **review** #7398 (arismoko) entity attachments for seat data | |
| C9 | Actors (drills, harvesters, ploughs) break and use | `BlockBreakingMovementBehaviour`, `HarvesterMovementBehaviour` | F6 commands; `create_use::use_item_at` | see F6, P3 | g 3 |
| C10 | Sync a contraption to a player who starts seeing it | `PlayerEvent.StartTracking` | own mod messages | **covered** `native_player_start_tracking` (141) | j 2 subs |

### Trains (tracks, stations, signals, carriages, schedules)

| # | Need | Create | Port today | Upstream | Use |
|---|---|---|---|---|---|
| T1 | Track graph and trains saved per world | `GlobalRailwayManager`, `RailwaySavedData` | `native_chunk_data`/`native_mark_chunk_unsaved` (ours 43); couplings via `native_saved_data` (ours 58) | **covered** attachments `Holder::Level`/`Chunk` (#6216, #7224) | g 4 |
| T2 | Carriages drawn moving | `CarriageContraptionEntity`, bogey renderers | parts compound and glide (ours 45/51) | **design** #6302 step 6 / #5313 | g 21 |
| T3 | Riding a carriage and driving it | `CarriageContraptionEntity`, `ControlsHandler` | seats (C6) + `input_key`/`native_is_key_down` (R6) + `interaction_key` (C5) | see C6, R6, C5 | g 27 |
| T4 | Station markers on maps | `RegisterMapDecorationRenderersEvent` | not ported | **covered** `native_map_decorations` (140) | |
| T5 | Track blocks with bezier models | `TrackBlock`, `BezierConnection` renderer | engine rules only (no glue yet) | **design** #6302 / #6303 (unverified which one fits) | |

### Decoration and copycats

| # | Need | Create | Port today | Upstream | Use |
|---|---|---|---|---|---|
| D1 | Copycat model per position (material, per-face state, cut boxes) | `CopycatModel`, `ModelData` | `dynamic_model` property (2 blocks) + `native_block_model_override` + mesher `model_override.rs` (ours 59) | **design** #6303 (levzzz5154, assigned). Clipping is not in its proposal | g 1, j 145 |
| D2 | Copycat sounds, light, friction and hardness of its material | `CopycatBlock.getAppearance`, `getSoundType` | none (the copycat's own) | **none** | j 2 |
| D3 | Connected textures (casings, windows) | `CTModel`, `CTSpriteShifter` | none | **none** | |
| D4 | Pick block gives the material or the right item | `getCloneItemStack` | not ported | **covered (partial)** `native_player_pick_item` (269, cancellable). Unverified whether it can replace the stack | j 39 |
| D5 | Invisible and BE-drawn blocks | `getRenderShape` | `render_shape` content key (76 blocks; engine `91ea0acdc`) | **none**: #5550 item 1 (JuanLuci, unclaimed) | j 15 |
| D6 | Own state properties beyond the template | `createBlockStateDefinition` | `state_properties` (298 files; engine `a83a4b380`, redeclares template props) | **covered** typed `state_properties` (#5306). Redeclaring a template property is refused on purpose (#5290) | 298 |
| D7 | Shapes per state, occlusion from shapes | `getShape`, `getCollisionShape`, `getOcclusionShape` | content `shapes` (176 files) + engine `95932161e` (shapes occlude) | **review** #7557 (Xmilon): multi-property `shapes` and `collision_shapes`; no occlusion switch | j 205 |
| D8 | OBJ and composite models (43 OBJ, 7 composite) | `neoforge:obj`, `neoforge:composite` | engine `9e557015d` OBJ loader | **review** #7568 (UMCEKO, OBJ), #7567 (UMCEKO, composite) | 50 |
| D9 | Placards, depots and display items show an item | `PlacardRenderer` | `native_part_item_model` (ours 45) | **covered** `native_draw_item` (91, immediate). Instanced form: **design** #6302 | g 8 |

### Rendering (client)

| # | Need | Create | Port today | Upstream | Use |
|---|---|---|---|---|---|
| RN1 | Instanced animated parts: set once, turned and slid on the GPU | Flywheel `Instancer`, `VisualizationManager` | `gfx/parts.rs`, `native_part_model/_set/_clear`, `_glide`, `_posed`, `_tinted`, `native_parts_seconds` (ours 42/45/48/50) | **design** #6302 (levzzz5154's agent; Ludovic on B/C, arismoko on `RenderHooks` vs hook; unclaimed). Immediate draws are **covered**: `native_block_entity_submit` (71), `BlockEntityDraw::keep` (94) | g 134, j 154 |
| RN2 | Partial models as meshes no blockstate names | `AllPartialModels` | 62 hidden `part_*` content blocks | **covered** `native_standalone_models`, `native_draw_model` (151) | 62 |
| RN3 | Per-instance vertex colour (analog lever indicator) | `SuperByteBuffer.color` (Catnip) | not drawn | **none** (raise on #6302) | |
| RN4 | Parts lit and relit across sections | Flywheel light | ours: brightest of the block and its 6 neighbours | **design** #6302 step 4 | |
| RN5 | Value-box and goggle outlines in the world | `ValueBoxRenderer`, `RenderHighlightEvent.Block` | HUD text only | **covered** `native_block_highlight` (86), `native_world_drawn` (58) | j 2 subs |
| RN6 | BE render culling bounds | `getRenderBoundingBox` | n/a (parts) | **covered** `native_block_entity_render_bounding_box` (71) | j 11 |

### Saving and world lifecycle

| # | Need | Create | Port today | Upstream | Use |
|---|---|---|---|---|---|
| S1 | Know when chunks load and unload | `ChunkEvent.Unload` | HEAD `chunk_load`/`chunk_unload(&Chunk)` (ours 41), fanned out to 9 subsystems | **covered** `native_chunk_load`/`_unload(dimension, [i32;2], …)` (57, #5131) | 2 hooks |
| S2 | Typed state saved with its chunk, dirtied on change | BE `write`/`read` | HEAD `chunk_data_save`, `native_chunk_data`, `native_mark_chunk_unsaved` (ours 43) | **covered** attachments `Holder::Chunk`/`BlockEntity` (#6216), `has`/`get_existing` (317). Unverified: whether a `set` marks the chunk unsaved | g 53, j 691 |
| S3 | World data loaded with the level | `LevelEvent.Load` | HEAD `level_load` (ours 58) | **covered** `native_level_load`/`_unload` (134) | |
| S4 | World data written at save | `LevelEvent.Save`, `SavedData.setDirty` | HEAD `level_save`, `native_set_saved_data_dirty` (ours 58) | **covered** attachments `Holder::Level` (saved by Horizon). A `LevelEvent.Save` hook: **none** (open #4953/fork#27 line). `native_world_saved` (306) fires after the save | g 2 |
| S5 | Data on entities (seat riders, carts by UUID) | `Entity.getPersistentData` | typed data keyed by cart UUID in level data | **review** #7398 (arismoko) attachments on every entity | |
| S6 | Files beside the game (schematics) | `FMLPaths` | `native_config_dir` (26) | **covered** (26), `native_instance_dir` (326) | g 1 |
| S7 | World open and close | `ServerStartingEvent`, `LevelEvent.Unload` | HEAD `world_lifecycle` (3) | **covered** (3), `native_server_about_to_start`/`_stopping` (44) | |

---

## 3. Trait-injection gaps (what the bundle still needs mixins for)

The bundle is a mixin bundle. Each item below is where it reaches Horizon internals that no hook, helper or handle exposes. The call site is where our `[[module]]` or HEAD injection sits, at `9d979ca`.

1. **All 30 glue modules are injected into `crates/horizon-client/src/game.rs`.** That gives them `Game`'s private fields, which they use 1,000+ times. Requests, in order of use:
   - **`game.world.block` (272) and `game.world.loaded` (12).** Please give #7559's `LevelRef::block_state` a **state-id form** (`BlockStateId`, or a typed key resolved once), beside the `String` form. Create reads states in per-tick loops: belts, pipes and the kinetic solve.
   - **`game.pack.assets.registry` (158).** Please add the fork#29 `Host::block_state_default` / state-by-name lookups to the registry. The point is resolving names to ids once at load.
   - **`game.world.game_time`/`day_time` (87).** Covered once #7559 merges.
   - **`game.res.behaviours`/`facts` (55).** Please consider the fork#29 `block_state_info` line (solid, replaceable, occludes) and `block_context_info`.
   - **Player fields (250):** `inventory`, `selected`, `hand`, `offhand`, `armor`, `creative`, `yaw`/`pitch`, `may_build`, `keys`. Covered by fork#30 and #7563 once merged. Inventory waits on capability parts. For `keys` (20) and our `native_is_key_down`, please add a client `Host` read for "is key down".
   - **`game.world.block_entities` (16).** Covered by fork#38 (read-only). Please also add a **write**, `set_changed`, from the fork#29 "block entities" line.
   - **`game.dropped` (via `items_in`, 11 callers).** Covered by `native_item_entities` (42), with motion from #7560 and fork#32.
   - **`game.pick()`/`aim()` (23).** Please add a server-side "targeted block of a player" (#5552 item 3).
   - **`game.with_level` (22).** 12 are covered by `native_neighbor_signal` (96). Please add a directional `signal(pos, dir)` read (6), and break-with-drops (2, see F6).
   - **`game.audio.play` (19, client).** Covered by `native_play_local_sound` (83).
2. **`create_extract` in `crates/horizon-client/src/game/machines.rs`.** It uses `containers()`, `slot_mut_at`, `view_changed` and hopper face rules, from 14 callers. Main's API 48 covers extracting. Please make **insert room per face** and **extract matching a filter** part of the capability-parts design (paused).
3. **`create_use` in `crates/horizon-client/src/game/dispenser.rs`.** It uses the private dispenser behaviours, `use_spawn_egg_on_spawner`, `targets_at` and `transform_particle`, from 8 callers. Please add a **fake player** that runs the real `use_item_on`/attack paths with its own inventory (#5556), so a deployer fires the same hooks a player does.
4. **HEAD injections on hook points that main lacks.** Each needs a hook, or a scoped hook under parts:
   - `update_entity_after_fall_on` (belts and depots catching items; `Block.updateEntityAfterFallOn`).
   - `minecart_pass` (`IBlockExtension.onMinecartPass`, controller rail).
   - `contents_changed` (`ItemStackHandler.onContentsChanged`, paused).
   - `seat_loaded`: a join hook for displays and other non-simulated entities loaded from disk. Main's `native_entity_join_level` skips them.
   - `level_save` (`LevelEvent.Save`).
   - `input_key` with `consumed`: a cancellable key event. Main's `native_input_key` (121) is "Not cancellable".
5. **Engine patches carried on `feat/create-hooks`.** These are not mixins, but the port does not run without them:
   - `gfx/parts.rs` (#6302);
   - the mesher's `model_override.rs` (#6303);
   - content keys `render_shape` (#5550), `dynamic_model`, `rail`, `entity_inside`, `analog_output` and `state_properties` redeclare;
   - occlusion from shapes (#7557 follow-up);
   - mod fluids (#6301);
   - the OBJ loader (#7568).

## 4. Summary

**Rows per category** (103 rows; a "(partial)" row is counted under its main category):

| Category | Rows |
|---|---|
| covered | 62 (9 of them partial: F3, F4, L7, FL2, E3, R3, R6, C3, D4) |
| none | 16 |
| design | 11 |
| review | 7 |
| paused | 5 |
| points to other rows | 2 (C9, T3) |

**Of our 27 Horizon API levels (41–67):**
- **Duplicated by main:** 41 → 57, 53 → 48, 56 → 64, 57 (listen half) → 121, 58 (load) → 134, 61 → 73, 63 (mount) → 137, 66 → 138. Persistence (43, 58 data) goes to attachments.
- **In review on main:** 60 and 64 (EntityRef and PlayerRef).
- **Still with no upstream line:** 47, 49, 54, 55, 57 (consume, `is_key_down`), 62, 65 (fuel), 67.
- **Waiting on designs:** the parts renderer (42/45/48/50/51) and 59.

**Top 10 gaps by importance to Create:**

| # | Gap | Why it matters to Create | Upstream |
|---|---|---|---|
| 1 | Instanced animated parts | Nothing kinetic visibly moves without it. 134 glue calls, Create's 243 `SuperByteBuffer` hits; belts, arms, trains and contraptions all draw through it | design #6302 |
| 2 | Capability parts (item and fluid handlers) | Funnels, chutes, arms, pipes and hoppers interoperate through 44 item and 44 fluid block capabilities (307 `IItemHandler` hits). It also needs per-item insert checks and contents-changed notices | paused (#7386) |
| 3 | Moving block structures | Bearings, pistons, pulleys, gantries, cart contraptions and every train are contraptions | design #5313 |
| 4 | Fake player | The deployer, and the drills, ploughs and harvesters on contraptions. Today a module sits inside `dispenser.rs` | none (#5556 unclaimed) |
| 5 | Typed block-state reads and the registry on `Host` | The most-used access (272 + 158 sites). The `String` form of #7559 is too slow for per-tick loops | review #7559 (string), none (typed) |
| 6 | Per-position block models | Copycats (and later connected textures); needs per-face state and clipping | design #6303 |
| 7 | Scoped own-block hooks (`updateEntityAfterFallOn`, `onMinecartPass`, `onPlace`/`onRemove`) | Belts and depots catching items, the controller rail. It would replace the hot `block_changed` broadcast | none (needs #7386 scope) |
| 8 | Riding non-mob vehicles and seats | Seats, carriages, and driving trains | none (fork#29 lines unclaimed; #4968) |
| 9 | World text drawing | Nixie tubes, display boards, train station displays | none (#5552 item 4 unclaimed) |
| 10 | Content-format keys | `render_shape` (76 blocks), occlusion from shapes (belts hide the floor otherwise), and redeclaring template properties (65 blocks redeclare `waterlogged`) | none (#5550, item 1 unclaimed); review #7557 (shapes) |

**Unverified items, to check before relying on them:**
- `reset_fall_distance` on any EntityRef line.
- Whether an attachment `set` marks the chunk unsaved.
- Whether `native_player_pick_item` can replace the picked stack.
- Whether `native_mod_screen` can host a text box.
- Whether `b133ffc29` (recipes in tick hooks) is still needed on main.
- Which of #6302 or #6303 fits track models.
