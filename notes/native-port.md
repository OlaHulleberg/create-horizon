# Porting the glue to the mod API

The mod used to be a horizon-mixins bundle whose glue (`legacy/glue/`, ~43k lines) called the
game's internals (`Game`, `game.world`, `game.native_*`). It is now a native mod: `src/lib.rs`
implements `mod_api::Hooks`, the glue under `src/glue/` reaches the game only through `mod_api`,
and the engine (`src/engine`, crate `create-engine`) is unchanged rules with no game types.
`legacy/glue/` stays only as the reference for what each subsystem did; it is deleted once every
subsystem is ported.

## Layout

| File | Job |
|---|---|
| `src/lib.rs` | The one `impl Hooks`: each hook written once, handed to every subsystem. The `CreateBlock` part every Create block carries. |
| `src/glue/subsystem.rs` | `trait Subsystem`: the moments a subsystem takes part in, all defaulting to nothing. |
| `src/glue/mod.rs` | `SUBSYSTEMS` (also the level tick order), column-load bookkeeping, the client's state table. |
| `src/glue/states.rs` | Create's states as engine ids (`FIRST..`), both ways; block keys (`states::key(Block)`). |
| `src/glue/world.rs` | The engine's `CreateWorld`, one per server thread (`with_world`), and level indices. |
| `src/glue/server.rs` | `ServerLevel`: one server level in engine terms (blocks as engine ids, signals, set/destroy). |
| `src/glue/persist.rs` | Each column's saved data: one chunk attachment holding a compound. |
| `src/glue/net.rs` | One payload, `create:net`, carrying the legacy channel names; outbox for backpressure. |
| `src/glue/kinetics.rs` | The `create:kinetic` block entity type, the network tick, speeds to clients, part drawing. |

## Translating legacy calls

| Legacy | Now |
|---|---|
| `thread_local` world, `with_world` | `glue::world::with_world` (same engine `CreateWorld`) |
| `dimension_index(dimension)` | `world::level_index(dimension)` (mod dimensions get indices too) |
| `game.world.block(x, y, z).0` | `lv.block(pos)` (engine id, 0 for any block not Create's) |
| `game.native_set_block_property_with(pos, k, v, flags)` | `lv.set_property(pos, k, v, flags)` |
| `game.native_set_block(pos, id)` | `lv.set_block(pos, id, flags)` |
| `has_neighbor_signal`, `best_neighbor_signal` | `lv.has_neighbor_signal(pos)`, `lv.best_neighbor_signal(pos)` |
| `native_run_command("setblock .. destroy")` | `lv.destroy_block(pos, true)` |
| `native_mark_chunk_unsaved(dim, chunk)` | `persist::mark_unsaved(dim, chunk)` |
| `chunk_data_save(.., &mut Compound)` / `Game::native_chunk_data(chunk, key)` | `Subsystem::chunk_data_save` / `Subsystem::chunk_load`: same compound, same keys |
| `native_net_send_to_player(p, channel, data)` | `net::send_to_player(host, player, channel, data)` |
| `net_from_client` / `net_from_server` match on channel | `Subsystem::client_message` / `server_message`, returning true for the channels it owns |
| `game.native_server_players()` filtered by dimension | `lv.players()` |
| `native_part_set` (instanced parts) | the block entity's `block_entity_submit` with `spin` (`kinetics::draw_part`) |
| HUD batches, `native_fill`, `native_text` | `DrawContext` in the HUD / screen hooks |
| Recipes from `game.res` JSON | `LevelRef::recipe_for`, the recipe hooks, or a server reload listener over `data/` |

A hook with no subsystem method yet: add the method to `Subsystem` and its one call in
`lib.rs`. A moment that belongs to one block, item or block entity is a Part method: block moments
on `CreateBlock` (dispatch by `Block`), block entity moments on the type's `BlockEntityPart`.

Per-object helpers go on the handle or facade they act on, in the subsystem's own module
(`impl ServerLevel<'_> { fn .. }` is allowed anywhere in the crate), not in `server.rs`.

## Rules

- Only `mod_api` and `create_engine`. No game internals, no `[[native.mixin]]`, no registry
  dependencies (the engine's vector maths is `create_engine::math`).
- Rules stay in the engine. The glue reads the world, calls the engine, writes back.
- Hot paths (block changes, ticks, per-block parts) return early for anything not Create's and
  allocate nothing when nothing changed.
- Saved data keeps the legacy layouts and keys, so the engine's `*_from_nbt` readers port as
  they are.
- Where the mod API has no way to do what the legacy glue did, leave that behaviour out, say so
  in the subsystem's module doc, and add a line to `HORIZON_API.md` under "Gaps found porting".
  No stubs, shims or game internals to cover it.
- Check with `cargo +1.99.1 check` (the SDK's toolchain), `cargo +1.99.1 test`, and
  `cargo +1.99.1 clippy --all-targets -- -D warnings`.
