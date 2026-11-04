# What the port needs from Horizon, and where it stands

Checked 2026-11-04 against the modding fork arismoko/horizon `main` (`da67617ff`). Re-check before acting on any
line: the fork merges dozens of PRs a day.

**Target: the fork's trait-mod API.** The fork is where the native mod API is built (its AGENTS.md, "the clean
break"; trackers #26 design, #44 milestones, #27 hooks), and contributors who merge upstream (levzzz5154) work there
too. The mod builds against one pinned fork commit with the fork's launcher (arismoko/horizon#44, answer of
2026-11-03). Upstream's answer on direction (horizon-rust/horizon#4953) is still pending.

The port today is a horizon-mixins bundle: a rules engine with no game types (`mixins/create/engine`, ~68k lines,
moves over unchanged) and game glue (~43k lines in 33 modules, 50 hook injections) that calls `Game` internals. The
fork forbids that: a mod depends only on `mod_api` and uses catalogued hooks, Parts, handles, capabilities under
transactions, attachments, mod messages and draw contexts. The glue is rewritten against those.

## What exists (port the glue to it)

| Area | Fork API |
|---|---|
| World and level life | `world_lifecycle`, `level_tick_post` (keep one global tick; a block entity's own tick runs with it out of the world), `tick` (`HostSide`), `client_level_unload`, `level_load` |
| Saved state | attachments with codecs on `Holder::Chunk` / `BlockEntity` / `Level`; a write marks the chunk dirty, so no save hook is needed |
| Blocks | `LevelRef::block_state` / `set_block` / `destroy_block` / `block_shape` / `block_face_sturdy` / `clip` / `level_event` / `play_sound` / `random`; `block_state_id` / `_string` / `_default` / `_property`; `LevelHost::state_with_property`, `fluid_state`, tags |
| Block behaviour | `BlockPart`: `use_item_on`, `use_without_item`, `state_for_placement` (yaw, pitch, sneaking, face, hit), `neighbor_changed` (read-only), `tick_block`, `block_signal`, `is_signal_source`, `has_analog_output` + `analog_output`, `entity_inside`, `contents_changed`, `affect_neighbors_after_removal` |
| Block entities | `r.block_entity_type(..).blocks(..)`, `BlockEntityPart::block_entity`, `block_entity_update_tag` + `block_entity_sync`, `screen` / `screen_draw` / `screen_click`, `BlockEntityRef::data` / `merge_data` / `replace_data` / `set_changed` |
| Items, fluids | `ITEMS`, `FLUIDS`, `ITEM_FLUIDS`, `ENTITY_ITEMS` providers, `move_resources`, `host.transaction`, `ItemPlace`. Create needs its own providers: vaults outgrow `block_entity_container`'s 256 slots, tanks `block_entity_fluids`' 1,000,000 mB |
| Items used | `ItemPart::use_on` / `use_item` / `hurt_enemy`, `right_click_item`, `left_click_block`, `entity_place`, `break_block`, `loot_drops`, `entity_interact`, `attack_entity`, `player_tick`, `living_breathe` |
| Entities | `LevelRef::entities_in` / `spawn_item(_with_motion)` / `pop_resource`, `summon`, `EntityRef` push, velocity, riding, teleport, rotation, discard, air; `MinecartRef` fuel; `entity_mount`, `entity_join_level`, `vehicle_moved`; `LivingRef` equipment, `hurt_and_break`, attribute modifiers |
| Recipes, commands, messages | `recipes_of_type` / `recipe_result` / `recipe_ingredient`, `server_data_loaded`; `register_commands` / `perform_command`; `net_send_to_player` / `_to_server`, `net_from_client` / `_from_server` |
| Client | `block_entity_submit` + `BlockEntityDrawContext` (block, item, cube, quad, rotate, partial, keep), `block_entity_render_bounding_box`, `standalone_models`, `RenderHooks::world_drawn`; `hud`, `screen_render_post` + `DrawContext`; mod screens; `mouse_scrolling`, `movement_input`, `interaction_key_mapping_triggered`, `mod_key`; `crosshair_target`, `camera` |

## Missing: small additions the fork's rules allow

Each needs a claim on its #44 / #27 row first, with the plan agreed in the thread, as levzzz5154 does.

1. **Catalogue helpers that already exist** in the game but not in `native-hooks.json`: `native_draw_model`
   (`native_block_entity_draw.rs:544`), `native_text_width`, `native_sprite_size`, `native_playing`,
   `native_mapping` (`native_hooks.rs`). One fragment; unblocks partial models and the train HUD.
2. **Redstone on `LevelRef`**: `signal(pos, dir)`, `best_neighbor_signal(pos)`, `update_neighbors(pos, NeighbourUpdate)`;
   removes the id-based `neighbor_signal`, `update_signal_neighbors`, `update_output_neighbors` (no shims).
3. **State facts off the world**: can be replaced, destroy speed, sound type, has block entity, the states of a
   block; `item_block` (row G9, host-lead-2: coordinate).
4. **Breaking with a tool**: `LevelRef::destroy_block` with breaker and tool, or `block_drops(pos, tool)`; break progress.
5. **Entity motion**: `EntityRef::reset_fall_distance`, `move_by` (`Entity.move`), `MinecartRef::set_push`.
6. **Fluid flow** (`FluidState.getFlow`) on the level host, for water wheels.
7. **Crafting-grid match** (`RecipeManager.getRecipeFor` over a `CraftingInput`) on `LevelRef`.
8. **Part methods** (NeoForge `IBlockExtension` / vanilla `Block`): `minecart_pass` (`onMinecartPass`),
   `set_placed_by` with the stack, items landing (`updateEntityMovementAfterFallOn`); with the F1/F2 rows (api-lead).
9. **Client**: `draw_text` on the block-entity draw context, GUI text with alpha, clipboard text, UI sounds without a
   position, block-entity view distance (`getViewDistance`), write access in screen hooks.
10. **Fake players F5/F6** (use, place, attack, break): rows 83–84 of #44, unblocked and unclaimed. The deployer
    depends on them.

## Large designs (contribute, don't work around)

- Instanced moving parts: #44 "Rendering" (not started), #27, horizon-rust/horizon#6302. Until then, per-frame
  `block_entity_submit` drawing.
- Moving block structures (contraptions, trains, carts; collisions, seats): horizon-rust/horizon#5313, #27.
  Draft: `notes/moving-structures-note.md`.
- Mod entity types (seats, carriages): #44 row E, horizon-rust/horizon#5212.
- A block entity reaching its own storage during its own tick: #44 (design accepted, not started).
- Copycat models in the chunk mesh: #44 D7, horizon-rust/horizon#6303.

## Not to send

- `8a8c23021` on `feat/create-hooks` (hopper item handler): written after reading a NeoForge source patch, which
  Horizon's rules forbid; the fork's `ITEMS` providers cover it.
- Block-facts snapshot cache `e4f3833ae`: a cache, kept private.
