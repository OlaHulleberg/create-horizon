# Porting Create 6.0.11 to Horizon (Nightly 9d979ca, MC 26.3)

Source of truth for behaviour: this repo's Java tree (`src/`, branch `main`, NeoForge 1.21.1); the port lives in `horizon/` on branch `port/horizon`.
Target: a horizon-mixins native bundle in `horizon/`, plus new hook points upstreamed to Horizon on
`/home/techi/repos/horizon` branch `feat/create-hooks` (based on `9d979ca`, `NATIVE_HOOKS_API` 40).

## Licensing

Create's licence (`LICENSE.md` at upstream Create; the file isn't in this repo's initial commit) splits the tree in two:

- **MIT: everything outside `src/main/resources/assets/`.** That covers all the Java code and everything under
  `src/generated/`, generated blockstates, models and lang included. Read it, translate it, copy it, change it
  as needed. The one condition: keep Create's copyright and MIT notice with the port (add it to this folder's
  `LICENSE` / `THIRD_PARTY_NOTICES` in phase 0).
- **All Rights Reserved: `src/main/resources/assets/`**: textures, hand-written models, sounds, shaders, ponder
  and its lang files. Don't commit or ship these. The prepare step reads them from the player's own Create
  jar at install time.
- Third-party libraries keep their own licences. Registrate (MPL-2.0) isn't needed. Horizon's CONTRIBUTING
  doesn't allow MPL/GPL/LGPL source, so stay out of it for anything that goes upstream. Check Flywheel's and
  Ponder's LICENSE before reusing either.

## Size

| Create subsystem | Java LOC | Files |
|---|---:|---:|
| content/kinetics (shafts, cogs, motors, press, mixer, fan, saw, drill, deployer...) | 36k | 254 |
| infrastructure (datagen, ponder scenes, config, gametests) | 33k | 131 |
| foundation (block entity behaviours, networking, GUI, utilities) | 32k | 324 |
| content/trains | 30k | 164 |
| content/logistics (belts, funnels, chutes, tunnels, packager, stock, frogport) | 30k | 219 |
| content/contraptions (moving structures, bearings, pistons, gantries, minecarts) | 27k | 185 |
| content/equipment (goggles, wrench, backtank, potato cannon, extendo grip...) | 17k | 141 |
| content/fluids | 12k | 76 |
| content/redstone | 12k | 113 |
| compat (JEI, CC, Curios, Sodium...) | 10k | 114 |
| decoration, schematics, processing, api, other | 26k | ~260 |
| **Total** | **275k** | **2017** |

Data: 643 blockstates, 741 item models, 114 block entity types, 9 entity types, 4 fluids,
1881 generated recipe files, 184 renderer/visual classes.

## What Horizon already gives (hooks API 40)

Mod blocks/items from vanilla templates (content mod data), block entities with ticks, slots and saves (15-18),
menus + custom screens with buttons/tooltips (17, 19, 20), custom recipe types (36), redstone signals (38),
shape updates / random & scheduled ticks (32-34), use/break/place/click hooks (13, 14, 21, 29, 30),
server<->client mod messages (40), HUD + tooltips (12, 27), mob helpers (37).

## Gaps that must become Horizon hook points first (branch `feat/create-hooks`)

Each is its own small Horizon PR (< ~500 lines, NeoForge-named per docs/native-hooks-neoforge.md, with a test):

1. **GPU-instanced part models** (client), not a per-frame draw callback: a mod registers part meshes
   once, then sets/clears per-instance records `{pos, axis, rpm, phase, light}` only when they change;
   the vertex shader turns each part from the frame time. Without it nothing in Create visibly moves.
   Biggest blocker. See "Performance architecture" below for why.
2. **Custom block models + shapes** beyond "copy a vanilla template": Create's JSON models with arbitrary
   elements, custom collision/outline voxel shapes, connected textures (casings).
3. **Custom entity types** with own renderer, collision and sync: contraptions (moving block assemblies
   the player can stand on), seats, super glue, potato cannon projectile, train carriages.
4. **Capabilities** (`block_capability(pos, kind, side)`, proposed in native-hooks-neoforge.md):
   item + fluid handlers so funnels, chutes, hoppers and pipes interoperate with vanilla and each other.
5. **Custom fluids**: registered fluid types with flowing blocks, buckets, rendering (honey, chocolate,
   potion fluid, tea).
6. **In-world item rendering** on belts/depots and **fake players** (deployer).
7. **World-space overlays**: goggle info, value-box hover outlines, schematic ghost blocks.
8. **Typed block entity state**: `chunk_load` / `chunk_unload` (in the native-hooks-neoforge.md table) plus a
   block entity save hook, so the mod keeps its state in its own typed arena and writes NBT only when the chunk
   saves. Today `ModBlockEntity` is `{id: String, nbt: Compound, items, data}` and is ticked through that
   compound (`game/native_hooks.rs` `tick_mod_block_entity`, `save.rs`), with no load/unload/save hook.

## Using Create's Java

The Java in `../src` is MIT: translate it directly wherever it fits, for rules, numbers, formulas and
algorithms (stress values in `CStress.java`, speeds in `CKinetics.java`, recipe processing, contraption
assembly, track geometry...). The only reason to depart from it is speed. Where Create's runtime structure is
slow (below), keep its behaviour and restructure the data. Where a fact is in both the data files and the
Java, read it from the data, which the build-time compiler can turn into tables.

## Performance architecture

The port runs no Java: no JVM and no Java oracle, at build time or at play time. Behaviour comes from
Create; the runtime layout is Horizon's. Four Create runtime patterns are replaced where they cost time: one
ticking object per block, behaviours looked up per tick, NBT as live state, and renderers rebuilding vertices
every frame.

### No Java, anywhere

- **Play time.** Everything is Rust that horizon-mixins compiles into the game: plain inlined calls,
  no FFI, no scripting, no reflection.
- **Data at build time.** Everything under `src/generated/` is MIT (about 7.2k files, data and generated assets:
  1881 recipes, 640 loot tables, 185 tags, 643 `.wiki/block_info` files with hardness, blast resistance,
  stack size, waterlogging, 643 blockstates, 1921 generated models). It can be committed and embedded. A small Rust tool, `create-data`, reads it once and writes a typed table:
  a `Kind` enum per Create block, and dense arrays for its properties, kinetic role and part models.
  The game does no JSON parsing for anything a data pack can't change.
- **Data a world can change** (recipes, tags, loot) stays data-driven. Vanilla recipe types go into the
  content-mod layer Horizon already loads. Create's own types (crushing, milling, pressing, mixing, ...: 20
  folders) come from API 36's `native_recipes_of_type`. They are parsed once per load or `/reload` into typed
  tables indexed by input item, and never read again as JSON during a tick.
- **ARR assets** (`src/main/resources/assets/`: 1294 textures, 841 hand-written models, sounds, shaders) are
  the only files that can't ship. A one-time prepare step reads them as a zip from the player's own Create jar. Reading a
  zip runs no Java. It writes the result into the content-mod resource layer, so Horizon bakes them into its
  atlas exactly as it does vanilla's, and the part meshes are baked once into GPU buffers.

### Kinetics: solve on change, never per block per tick

- No block entity tick for any passive kinetic block (shaft, cog, gearbox, belt, clutch...).
- A per-dimension `KineticWorld`: nodes in a slot map, with speed, direction, stress and capacity in
  struct-of-arrays columns. Adjacency is keyed by chunk section. Networks are tracked as connected
  components.
- `native_block_changed`, `native_place_block`, `native_destroy_block` and the redstone hooks mark a
  network dirty. The solver re-runs only that network, once per tick at most, from `level_tick_post`.
  A steady factory costs nothing per tick.
- Machines that do work each tick (press, mixer, saw, fan, deployer, drill) sit in a dense active list,
  which is ticked in a tight loop. Machines with no speed or no input leave the list.
- Rotation angle is never simulated. It is `rpm * time + phase`, computed by the GPU.

### Rendering: instanced, uploaded on change

- Horizon's own block entity renderers build vertices on the CPU every frame
  (`horizon-client-render/src/block_entity_render.rs`, `gfx/entity_batch.rs`). That is fine for chests,
  not for a factory of thousands of shafts.
- Create's moving parts use the instanced hook (gap 1). Meshes are uploaded once per part model. An
  instance record is written only when a speed, block or light changes. One draw per part model per frame.
  The renderer already uses instanced vertex layouts (`gfx/native_material.rs`).
- Static parts (casings, frames) stay in the chunk mesh like any block.
- Contraptions are baked once into a mesh, a small chunk, and drawn with one transform per frame. Items on
  belts are instances, not item entities.

### Hot hooks

- Hot hooks (`block_changed`, `neighbor_changed`, `update_shape`, `block_signal`, `tick_block`) take
  `BlockStateId`. At world open the mod builds a bitset of Create's state ids. Each hook's first line is one
  bit test, then return. It never compares strings, never allocates, never hashes on the miss path. The hook
  docs say these run "very often, keep it short".
- Block-entity hooks pass names as `&str`. Resolve a name to a `Kind` once, at world open or when the block
  entity is created, never per tick.

### Logistics, fluids, trains

- **Belts:** one chain per connected belt line, with items as a sorted `Vec` of (offset, stack), stepped in
  one loop. Funnels and tunnels act at chain indices. No item entities.
- **Pipes:** pipe networks are graphs, re-solved on change. Flow steps only along active pressure paths.
- **Trains:** the track network is a graph with signal blocks per edge. Only trains that are moving
  simulate. Carriages are contraptions (baked meshes).

### Measured, not assumed

- Each subsystem gets a benchmark world, measured with Horizon's `--benchmark` and `tools/perf_bench.py`
  ([docs/benchmark.md](../../horizon/docs/benchmark.md)): tick time and frame time, with mods off, with
  Create on and idle, and with Create under load.
- **Free when unused** (Horizon's rule, docs/modpacks.md): with no Create block in the world, every hook
  is a bit test and returns, and the tick and frame times match the run with mods off.
- Record budgets in this file after the first baseline. A change that regresses a budget is not done.

## Phases (each ends playable and committed)

0. Setup: rename template (`create`), repin to `9d979ca`, stage+check+test green. Build `create-data`,
   the build-time data compiler (below), and the asset prepare step from the player's Create jar
   (nothing from `src/main/resources/assets/` in git); add Create's MIT notice. Add a benchmark world and record a baseline with Horizon's `--benchmark`.
1. Static content: every decoration/material block and item (andesite alloy, brass, casings, stone types,
   windows, ...) with models, loot, tags, vanilla-type recipes.
2. Kinetic core: rotation propagation graph, speed/direction, stress units & capacity, overstress;
   shaft, cogwheels, gearbox, clutch, gearshift, encased chain drive, belts (transmission), creative motor,
   water wheel, windmill, hand crank. Needs hook gap 1 for visuals.
3. Processing: millstone, crushing wheels, press, mixer + basin, encased fan (washing/smoking/blasting/
   haunting), saw, drill, deployer, blaze burner, mechanical crafter, all Create recipe types (API 36).
4. Logistics: belts carrying items, depot, funnels, chutes, smart chutes, tunnels, filters, arms,
   item vaults, packager/stock ticker/frogport.
5. Fluids: pipes, pumps, tanks, spout, item drain, hose pulley, custom fluids.
6. Equipment + redstone: wrench, goggles, backtank, diving gear, potato cannon, extendo grip; links,
   nixie tubes, display links/boards, analog levers, pulse repeaters.
7. Contraptions: mechanical bearing, piston, rope/elevator pulley, gantry, cart assembler; moving blocks,
   collision, actors (drills/saws/harvesters on contraptions), schematics + cannon.
8. Trains: tracks (bezier), stations, signals, schedules, carriages, network graph.
9. Polish: Ponder-equivalent help, recipe viewer via item panel (API 2), configs (API 26), multiplayer
   (LAN + dedicated once Horizon builds native mods there).

## Verification per feature

- Unit tests in `runtime.rs` for pure logic (kinetic solver, recipe matching, stress math).
- `python3 tools/horizon.py check|test|build --horizon ../horizon` green.
- In-game proof: a scratch-HOME run (see Horizon AGENTS.md "keep runs away from real player data"),
  driven with `horizon-ctl`, screenshot or state dump showing the feature behaving as Create does.

## Paths

`tools/horizon.py`'s default `--horizon` is `ROOT.parent / "horizon"`, which here is this folder itself:
always pass `--horizon /home/techi/repos/horizon`.
