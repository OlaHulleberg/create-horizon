# Architecture

Create as a native Rust mod on the mod SDK (`mod_api`). Paths are under `create/` unless noted.

## Layout

| Path | What it is |
|---|---|
| `horizon-mod.toml` | Identity, the pinned Sunrise build, Create's jar (`[[jar]]`, by SHA-512). |
| `src/lib.rs` | The one `impl Hooks`. Each hook hands off to every subsystem; every Create block carries the `CreateBlock` part. |
| `src/engine/` | `create-engine`: Create's rules as plain Rust, no game types, unit-tested. |
| `src/glue/` | Game glue on `mod_api`, one module per subsystem. |
| `data/`, `assets/` | Generated block, item and fluid declarations and partial-model block states. |
| `../tools/create-data/` | `compile`: Create's MIT data → `data/`, `assets/`, `src/engine/src/generated.rs`. `prepare`: this repo + Create's jar → a playable mod folder. |

## Engine and glue

- **Engine** (`create_engine`): Create's behaviour, ported from its Java (kinetic networks, belts, recipes, fluids,
  contraptions, trains, schedules...). Blocks are engine ids (`create_engine::Block`); one `CreateWorld` holds each
  level's state. No `mod_api`, so everything is testable with `cargo test -p create-engine`.
- **Glue**: reads the world through `mod_api`, calls the engine, writes the result back (block states, block
  entity data, sounds, particles, draws). Rules never live in the glue.

Core glue modules:

| Module | Job |
|---|---|
| `glue/subsystem.rs` | `trait Subsystem`: every moment a subsystem can take part in, defaulting to nothing. |
| `glue/mod.rs` | `SUBSYSTEMS`, in level-tick order (also the order shared moments ask them in). |
| `glue/states.rs` | Create's block states ↔ engine ids; block keys (`states::key(Block)`). |
| `glue/world.rs` | The engine's `CreateWorld` (`with_world`) and level indices. |
| `glue/server.rs` | `ServerLevel`: a server level in engine terms (blocks as engine ids, redstone, set/destroy). |
| `glue/persist.rs` | Per-column saved data in one chunk attachment. |
| `glue/net.rs` | One payload, `create:net`, with named channels; an outbox for backpressure. |
| `glue/kinetics.rs` | The `create:kinetic` block entity type, the network solve, speeds to clients, part drawing. |

## Subsystems

Each module under `glue/` is a subsystem: kinetics and transmission, blocks, redstone, display, contraptions
(bearings, pistons, gantries, lifts, seats, controllers, drawing), processing, fluids, trains and carts,
interaction (value boxes, cranks), logistics (belts, funnels, chutes, tunnels, arms), gauges, sources (stress,
ambience), packages, stock, schematics, decor, copycats, items and equipment, advancements.

## Data flow

1. **Registration.** `lib.rs` registers states, the `CreateBlock` part on every block, the network channels, then
   each subsystem's parts, block entity types and commands.
2. **Changes.** Each subsystem hears block changes (`block_changed`), the kinetic world first.
3. **Level tick.** `SUBSYSTEMS` run in order: inputs (redstone, cranks), machines, fluids, trains, logistics, then
   the kinetic solve, then what reads its result (gauges, comparators).
4. **Clients.** The server writes what clients draw into block entity data (synced keys) or sends it on
   `create:net`; client hooks and block entity parts draw it.
5. **Saving.** Each subsystem writes its column state in `chunk_data_save` and reads it back in `chunk_load`;
   block entity data saves with the block entity.

## Adding a block

1. Make sure `tools/create-data compile` declares it (`tools/create-data/src/blocks.rs` picks the vanilla
   template whose properties and shapes it needs); regenerate, never edit `data/`, `assets/` or `generated.rs`.
2. Put the rules in the engine, with tests.
3. In the subsystem's glue: handle its moments (`Subsystem` methods, or a `BlockPart` / `ItemPart` /
   `BlockEntityPart` on its own block or item), call the engine, write back.
4. If the API can't do part of it, leave that part out, say so in the module doc and add an entry to
   [API_GAPS.md](../API_GAPS.md). No stubs, shims or game internals.

## Rules

- Only `mod_api` and `create_engine`: no game internals, no `[[native.mixin]]`, no registry dependencies.
- Hot paths (block changes, ticks, per-block parts) return early for anything not Create's and allocate nothing
  when nothing changed.
- Per-object helpers go on the handle they act on, in the subsystem's own module.
- A new hook: add a method to `Subsystem` and its one call in `lib.rs`.
