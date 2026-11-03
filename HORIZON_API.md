# What the port needs from Horizon, and where it stands

Checked 2026-11-03 against horizon-rust/horizon `main` (hook API 359) and the modding fork arismoko/horizon `main`
(where native-mod API work goes, horizon-rust/horizon#4953; tracker arismoko/horizon#27). Re-check before acting on
any line: both move by the hour. Our old hook branch is `feat/create-hooks` on OlaHulleberg/horizon (65 commits on
`9d979ca`); it is a source of ideas, not something to send as is.

The goal: the mod builds on the Horizon the team releases. That means the trait-mod format (`horizon-mod.toml`
`[native]`, `src/lib.rs` implementing the generated `Hooks`) using catalogued hook points and Host helpers only; raw
injections into internal functions break on every refactor.

Up = horizon-rust main, Fork = arismoko main. a = has it, b = partly or differently, c = missing.

## Use what exists (port the mod to it)

| Need | Up | Fork | Use |
|---|---|---|---|
| Block `state_properties` | a | a | content declarations (horizon-rust#5306) |
| `render_shape`, `occludes` | a | a | ours, merged upstream (#7867, #8083) |
| OBJ models | a | a | `neoforge:obj` (horizon-rust#7568) |
| Saved state with chunks / levels / block entities / entities | a | a | attachments, `AttachmentType` + `Holder` (#6216); fork also `level_save` |
| Comparator output | a | a | `has_analog_output`, `analog_output`, `update_output_neighbors` |
| Item entities in a box | a | a | `item_entities`, `set_item_entity` |
| Taking items out of containers | a | a | `extract_item`, `move_items`, `container_slots`; fork ITEMS + `move_resources` |
| Hoppers, funnels into a mod block (item handler) | c | a | fork ITEMS capability providers; `block_entity_capability_before_game` orders ours first |
| Contents changed | b | a | fork ITEMS providers see every insert and extract |
| Interaction key | a | a | `interaction_key` |
| HUD layers | a | a | `gui_layer_pre`/`gui_layer_post` |
| Recipe folders, recipe helpers on servers | a | a | `json_folder`, `outside_world()` |
| Entity inside a block | b | b | `block_entity_inside` (static: act next tick) |
| Item landing on a block | b | b | `block_step_on`/`block_entity_inside` + `item_entities` (next tick) |
| Mod screens | b | b | `mod_screen_open`/`close`, `screen_render_post`, `screen_*_pre`, `screen_closing` |
| Riding, seats | b | b | `entity_mount`; fork `entity_start_riding`/`stop`/`passengers` |
| Minecarts | b | b | `summon`/`remove_entity`, `vehicle_*`; fork `minecart_set_fuel` |
| Entity reads and moves | b | b | fork `level_entities_in`, `entity_push`, `entity_set_rotation`, `entity_teleport_to`, `entity_is_shift_key_down` |
| Drawing machines | b | b | `block_entity_submit` + `draw_block`/`draw_model`/`draw_item`, `draw.keep()` |
| Chunk load / unload | b | b | `native_chunk_load` + `read_columns`; fork `chunk_refs` |

## Missing: candidates for small PRs (each needs a claim first)

Fork (mod API):
1. `native_recipes` on the server's tick (still `self.player.as_ref()?` on both; ours `578bc1aae`).
2. Fluid flow read (`FluidState.getFlow`) as a level helper.
3. Item draws lit as a block (`isGui3d`).
4. Clipboard text helpers; `mod_screen_open` that pauses (new version).
5. Entity gaps: reset fall distance, is-flying, velocity on players.
6. Host or deferred world access in `block_entity_inside` / `block_step_on`, or confirm next-tick is intended.
7. A deployer using its item on the block it reaches (`use_item_at`).
8. `onMinecartPass` as a block-scope part, and mod rails.
9. Directional signal and neighbour-update helpers, if still needed.

Upstream (engine, content format):
1. Declared shapes as occlusion shapes (ours `95932161e`).
2. Faster block tables (ours `1754ad635`).

## Contribute to existing designs instead of new hooks

- Instanced moving parts and world text: horizon-rust#6302 (partly done).
- Model changes at bake time: horizon-rust#6303.
- Mod fluids: horizon-rust#6301 (water/lava only today).
- Moving block structures (contraptions, trains, carts; entity collisions, seats): horizon-rust#5313.

## Not to upstream

- `8a8c23021` (hopper item handler before the container): written after reading a NeoForge source patch, which
  Horizon's rules forbid; the fork's capability providers already cover it.
- Block-facts snapshot cache `e4f3833ae`: a cache, kept private.
