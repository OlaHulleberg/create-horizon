# Porting Create 6.0.11 to Horizon (Nightly 9d979ca, MC 26.3)

Source of truth for behaviour: this repo's Java tree (`src/`, branch `main`, NeoForge 1.21.1); the port lives in `horizon/` on branch `port/horizon`.
Target: a horizon-mixins native bundle in `horizon/`, plus new hook points upstreamed to Horizon on
`/workspace/horizon` branch `feat/create-hooks` (based on `9d979ca`, `NATIVE_HOOKS_API` 40).

## Status (updated with every commit)

| Phase | State | Verified | Measured |
|---|---|---|---|
| 0 Setup | done: bundle `create` on 9d979ca, `create-engine` crate, glue module, `create-data compile/prepare`, MIT notice, perf baseline | stage/check/test/build green; fixture test; headless launch on BMG G21: content loads, mod idles without it | see Budgets |
| 1 Static content | in progress: 400 of 643 blocks and 35 plain items declared from exact vanilla templates; 1369 recipes, loot, tags, models, textures, lang from the prepare step | headless screenshot: casings, metal blocks, stone palettes, windows, shingles, stairs, slab, ladder, pillar, wall, scaffolding render with Create's textures | idle cost: see Budgets |
| 2 Kinetic core | in progress: solver (`kinetics.rs`: Create's ratios for shafts, gearbox faces, small/large cogs, split shafts; overpowering, direction conflicts, 256 RPM limit, stress/capacity from CStress), block→part table (`parts.rs`), per-world model (`world.rs`), glue on `block_changed` (hot: two range tests), `chunk_load`/`chunk_unload`, `level_tick_post` (solve only when dirty), `/create_debug speed`; shaft, cogwheels, gearbox, encased shafts/cogs, clutch, gearshift, creative motor, hand crank declared. **Not yet:** rotation visuals (gap 1), motor/crank interaction, redstone on clutch/gearshift, water wheel, chain drives, belts | engine 21 tests; fixture drives every hook into the engine; headless: motor 16 rpm → shafts 16 → gearbox corner +16 → cogs 16/−16 → large cog 8; unload/reload rebuilds the network; opposing motors break the newer one; screenshot of the build | pending |
| 3-9 | not started | | |

### How the port is built

- `mixins/create/engine` (`create-engine`): Create's rules as plain Rust with no game types, staged into Horizon's
  workspace as a bundle crate (horizon-mixins 0.6 `[[crate]]`); its tests also run there (`horizon.py test`).
- `mixins/create/runtime.rs`: the glue module `crate::game::create`, per-world state on the server thread.
- Content: Horizon content-mod layer, namespace `create`. `create-data compile` writes the block/item declarations
  (`content/`, committed: our own files) and `engine/src/generated.rs` (one `Block` enum over all 643 blocks, sorted as
  Horizon registers them). `create-data prepare <jar|folders>` writes the playable content mod: declarations + Create's
  MIT data converted to 26.3 (`{"item": x}` ingredients become `"x"`, `neoforge:conditions` naming other mods drop the
  recipe) + Create's assets from the player's jar + the `assets/create/items/*.json` model definitions 26.3 needs.
- Hot-path test: Create's states are one contiguous id run (mods are appended in id order), so "is this a Create block"
  is one load, one subtract, one compare on an `AtomicU64` (`create_engine::states::index`). Without the content mod the
  run is empty and every test misses.

### Budgets (measured on this sandbox: Ryzen 7 5700X, Arc B580-class BMG G21, Mesa 25.0.7 Vulkan, offscreen)

`tools/perf_headless.sh` runs Horizon's `perf_bench.py --offscreen` under Xvfb with scratch homes: 1280x720, render
distance 12, seed 1234, 3 repeats, the four scenes; `--warm` excludes the one-time content-overlay build. Reports
in `benchmarks/perf/`, compared with `tools/perf_table.py`. A difference inside the two runs' 95% CIs is noise (~).

| Metric (warm) | Mods off | Create content on, no Create blocks | Budget |
|---|---|---|---|
| Frame rate, 4 scenes (avg fps) | 1762 / 1570 / 1752 / 1557 | -1.3% ~ / +0.3% ~ / -0.4% ~ / +0.2% ~ | within CI of mods off |
| 1% low fps, 4 scenes | 772 / 574 / 869 / 665 | all ~ (inside CI) | within CI of mods off |
| Chunk load, 4 scenes (ms) | 2400 / 2845 / 2152 / 3743 | all ~ | within CI of mods off |
| Start to title (ms) | 845 ± 5 | 1360 (+515) | **open**: content cost, see below |
| Start to world (ms) | 3748 ± 29 | 4651 (+903) | **open** |

Server tick time during flights is recorded per scenario by `tools/bench_create.py` from Phase 2 on.

### Blocked / open

- Startup with Create's content installed: +0.5 s to the title screen and +0.9 s into a world (warm). The native mod
  does nothing at startup but scan the registry once; the time is Horizon reading Create's content: 25,349 more block
  states (56 walls are 18,144 of them), 5,557 asset/data files and Create's textures in the atlas. Not yet
  "no measurable cost": to investigate (fewer states via gap 2 own states for walls, or a faster content load in Horizon).

- Horizon pin: the mod now builds on `feat/create-hooks` (`0a96593cf`, two commits on 9d979ca): gap 2
  (`a83a4b380`, content blocks with their own `state_properties`) and gap 8 (`0a96593cf`, hook points API 41:
  `chunk_load`/`chunk_unload`). Horizon's `pr_ready --quick` is green for gap 2; for gap 8 every check is green except
  `horizon-client-render`'s parallel GPU tests, which segfault in this sandbox on untouched 9d979ca too (2 of 3 runs);
  they pass single-threaded (700/700).
- Loot: Create's 1.21.1 tables name a condition's kind `condition` and list `functions`; 26.3 (and Horizon) read
  `type` and `modifier`. `create-data prepare` converts them; before, every Create block dropped nothing.
- Horizon's `/loot ... mine` looks up `minecraft:blocks/<name>` even for mod blocks, and `/loot loot <id>` checks a
  vanilla-only id list: both refuse Create's tables (Horizon issue; drops themselves work).
- `horizon-ctl command` runs as the server console, which does not reach mod commands; drive `/create_debug` with
  `horizon-ctl chat "/create_debug ..."`.
- Gap 1 (instanced part rendering) is next: nothing rotates on screen yet.
- Zinc ore generation: Create adds it with a NeoForge biome modifier, which Horizon's content mods do not read.
- Copper shingles/tiles do not weather yet (template `minecraft:andesite`, copper sounds).
- Food items (bar of chocolate, sweet roll, honeyed apple, chocolate glazed berries) wait for Phase 6.

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
  (Horizon's `docs/benchmark.md`): tick time and frame time, with mods off, with
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
always pass `--horizon /workspace/horizon`.
