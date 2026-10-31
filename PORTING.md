# Porting Create 6.0.11 to Horizon (Nightly 9d979ca, MC 26.3)

Source of truth for behaviour: this repo's Java tree (`src/`, branch `main`, NeoForge 1.21.1); the port lives in `horizon/` on branch `port/horizon`.
Target: a horizon-mixins native bundle in `horizon/`, plus new hook points upstreamed to Horizon on
`/workspace/horizon` branch `feat/create-hooks` (based on `9d979ca`, `NATIVE_HOOKS_API` 40).

## Status (updated with every commit)

| Phase | State | Verified | Measured |
|---|---|---|---|
| 0 Setup | done: bundle `create` on 9d979ca, `create-engine` crate, glue module, `create-data compile/prepare`, MIT notice, perf baseline | stage/check/test/build green; fixture test; headless launch on BMG G21: content loads, mod idles without it | see Budgets |
| 1 Static content | in progress: 400 of 643 blocks and 35 plain items declared from exact vanilla templates; 1369 recipes, loot, tags, models, textures, lang from the prepare step | headless screenshot: casings, metal blocks, stone palettes, windows, shingles, stairs, slab, ladder, pillar, wall, scaffolding render with Create's textures | idle cost: see Budgets |
| 2 Kinetic core | in progress: solver (`kinetics.rs`: Create's ratios for shafts, gearbox faces, small/large cogs, split shafts; overpowering, direction conflicts, 256 RPM limit, stress/capacity from CStress), block→part table (`parts.rs`), per-world model (`world.rs`), glue on `block_changed` (hot: two range tests), `chunk_load`/`chunk_unload`, `level_tick_post` (solve only when dirty), `/create_debug speed`; turning parts (gap 1): the server sends changed speeds after a solve (`create:kinetics` mod messages, `client.rs` wire format; a joining client asks for all with `sync`), the client shows Create's models as GPU-turned instances (`client.rs`: own model for shafts/cogs, shaft halves for gearbox faces, motor and split shafts; Create's 22.5°/11.25° offsets); shaft, cogwheels, gearbox, encased shafts/cogs, clutch, gearshift, creative motor, hand crank declared. motor speed by mouse wheel on the motor (Create 5's scroll; Create 6's value board needs the overlay gap), saved with its chunk through API 43 (`chunk_data_save`, `horizon:mod_data`) and read back on load — proven: scrolled 16→32 rpm, saved, quit, relaunched, 32 rpm. clutch/gearshift follow redstone (`neighbor_changed`, hot: one range test; power applied after the level tick, before its solve: at most one tick later than Create) — proven: clutch powered 0 rpm / unpowered 16, gearshift powered −16. hand crank: right-click turns it 10 ticks at 32 rpm (sneak reverses), Create's food exhaustion, handle and shaft half drawn as turning parts (handle partial `part_hand_crank_handle`) — proven: 0 → −32 rpm on use, 0 again after 10 ticks; screenshots of the handle turning. water wheel: Create's flow score from the four blocks beside it (`waterwheel.rs`, after `WaterWheelBlockEntity` and Catnip's `VecHelper.rotate`), re-checked when a neighbour changes and every 60 ticks staggered by position, through API 44 `native_fluid_flow` (bubble columns as Create reads them); its OBJ wheel turns as a part (`part_water_wheel`) — proven: dry 0, water falling past the north side −8 rpm, both sides 0; screenshots of the wheel turning. **Not yet:** windmill, chain drives, belts, goggles' overstress display | engine 26 tests; fixture: chunk scan + hot hook + solve + server→client message → part instance at 16 rpm; headless: motor 16 → shafts 16 → gearbox +16 → cogs 16/−16 → large cog 8; unload/reload; opposing motors break the newer; two screenshots 0.7 s apart show the parts turning | see Budgets |
| 3 Processing | in progress: `processing.rs` (Create's processing recipes typed and indexed by input item, read once per world through API 36; `ProcessingOutput` rolls; `insertItemStacked`); millstone: small cog with a shaft below (impact 4), barrel-template block entity with 10 slots (hoppers in to slot 0, out of 1–9), Create's `MillstoneBlockEntity.tick` only for turning millstones, empty-hand use takes the products, turning stone drawn as a part. depot (`depot.rs`, after `DepotBehaviour`, `SharedDepotBlockMethods`, `DepotRenderer`): its stacks are the block entity's 9 slots (held 0, outputs 1–8; hoppers fill it only while it holds nothing, through API 46 `only_when_empty`, and take from every slot), the held stack's motion is typed state on the server; a right-click on its top gives back everything and puts down the hand's stack, sliding in from the player's side (`tick` only while it slides: an idle depot costs nothing); clients get its contents on `create:depot` (on change, on load, on join) and draw each stack as GPU item parts (API 45 `native_part_item_model`/`native_part_glide`, API 47 `native_item_uses_block_light`): Create's poses (flat items laid down, block items standing, a copy per two doublings, outputs 0.35 out at 45° steps with `new Random(i + 1)` angles), the slide run on the client tick by tick and glided between ticks on the GPU. mechanical press (`press.rs`, after `PressingBehaviour`, `BeltPressingCallbacks`): a shaft along its facing (impact 8); a depot two below asks it when its stack arrives or changes and when the press finishes or starts turning (an idle press or depot ticks nothing); the 240-tick cycle at Create's `getRunningTickSpeed`, the depot's stack locked while held, one item pressed at the head's lowest point (the whole stack when it is one: the first result becomes the held stack), results stacked into the outputs, overflow dropped above the depot; clients get each cycle on `create:press` (start, head down, done) and run it on, the head (partial `part_press_head`) moving on Create's curve, glided between ticks. Recipes from `create:pressing` (reloaded when `/reload` builds a new table). **Not yet:** pressing items lying in the world (Create's entity scan) and in basins (compacting), sequenced assembly, particles and sounds. **Not yet:** saving the millstone's timer and the depot's angle (an angle is 0 after a reload), items falling onto them, Catnip's `nudge`, upright items facing the camera, sounds, the other machines | engine 42 tests; fixture: depot put down → one `create:depot` message → the client glides the stack in from the south; headless: hopper of wheat above a millstone on a 16 rpm motor: 2 wheat milled in 18 s (150 ticks each), flour and seeds in its slots, taken by hand; depot: 16 iron put down by hand (block data `Items: [{count: 16, Slot: 0b}]`, hand empty), drawn as three flat copies; a hopper above a second depot moved one bread in and no more (the hopper kept 2 bread, 2 apples); an empty hand took the 16 iron back; press: creative motor → press at 16 rpm over a depot holding 5 iron: 4 iron + 1 sheet after 3 s, 3 + 2 after 13 s (one per 6 s cycle: 240 cycle ticks at 2 a tick), the head drawn lower mid-stroke; fixture: motor → press over a depot, one of five ingots pressed in 80 ticks | pending |
| 4 Logistics | in progress: mechanical belt (`belt.rs`, after `BeltBlock`, `BeltBlockEntity`, `BeltHelper`, `BeltInventory`, `BeltRenderer`): chains found from block states as Create's `initBelt` walks them (rebuilt when a segment is placed or its column loads), segments of one belt turn together (`propagateRotationTo`: same controller, ratio 1), pulleys carry the shaft; the items are a plain list on the controller, ticked only while the belt holds items (`BeltInventory.tick`: a block of spacing, Create's endings — into a depot or another belt, blocked by a sturdy face, thrown off — and presses over a segment holding and pressing items); items arrive as dropped items landing on it (API 49 `update_entity_after_fall_on`, also into empty depots), an empty hand takes the items of a segment; breaking a segment drops the belt's items and turns the next segments into shafts or air, chain by chain (`onRemove`). Clients get a belt's items on `create:belt` when they change and run the same tick; each item is a GPU item part gliding at the belt's speed (flat items turned by their angle on the GPU, sloped ones baked with their tilt), uploaded again only when its motion changes; segments draw Create's belt partials posed by its renderer's turns (API 48 `native_part_model_posed`), the pulleys turning. **Not yet:** the belt connector item (belts are placed by commands), the belt texture's scrolling, casings, tunnels, funnels, chutes, entities riding belts, belt saving (items on a belt are not saved yet) | engine 54 tests; fixture: a motor turns a 3-long belt, 4 gold ingots landing on its start ride it onto the depot past its end; headless: creative motor → 6-long belt south at 16 rpm (items 0.0333 blocks a tick: 16/480) → depot: 5 iron and 2 oak logs dropped on the start, the iron ends on the depot (block data `count: 5`), the logs wait at the end while the depot is full, no item entity left; screenshots of the stacks riding the belt, a top-down shot aligned between reference blocks | pending |
| 5-9 | not started | | |

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

`tools/bench_create.py` (through `/workspace/headless.sh`, uncapped frame rate, VSync off, 12 s flight over the
scene, 3 runs; the server thread's smoothed tick time over the flight). Reports in `benchmarks/create-*.json`.

The scene is built 30 blocks above the spawn point and checked before measuring (the driver counts its Create blocks
in 16-row bands: a single `horizon-ctl blocks` reply lists at most 4,096 blocks). `chains` is the control for
`kinetic`: the same layout in vanilla iron blocks and iron chains (the shaft's template).

| Scenario (2026-10-31, Phase 2, 25 s settle) | fps | frame p50 / p99 (ms) | server tick (ms) | Budget |
|---|---|---|---|---|
| off: no content mod | 1841 ± 157 | 0.530 / 1.028 | 4.047 ± 0.062 | baseline |
| idle: Create content, no Create blocks | 1944 ± 29 | 0.500 / 0.908 | 4.011 ± 0.040 | within noise of off |
| chains: the kinetic layout in vanilla blocks | 1935 ± 72 | 0.502 / 0.914 | 4.167 ± 0.083 | control |
| kinetic: 64 motors × 63 shafts, 4,096 GPU-turned parts, steady | 1856 ± 29 | 0.528 / 0.892 | 3.970 ± 0.058 | frame p50 ≤ chains + 10%, tick ≤ off + 0.25 ms |

A steady network costs no server tick (3.97 ms against 4.05 off and 4.17 for the vanilla control); 4,096 turning parts
cost 0.026 ms of frame time over the same blocks chunk-meshed. An earlier run of this table measured an empty scene
(the scene's commands had failed at unloaded positions); the driver now refuses to measure a scene it could not build.

| Scenario (2026-10-31, Phase 3, 25 s settle, same session) | fps | frame p50 / p99 (ms) | server tick (ms) | Budget |
|---|---|---|---|---|
| off: no content mod | 1942 ± 6 | 0.496 / 1.045 | 4.149 ± 0.034 | baseline |
| idle: Create content, no Create blocks | 1940 ± 41 | 0.502 / 0.897 | 4.051 ± 0.049 | within noise of off |
| processing: 256 presses at 16 rpm pressing 256 depots of iron (16 motors, 528 Create blocks) | 1787 ± 39 | 0.547 / 1.014 | 4.357 ± 0.023 | tick ≤ off + 0.5 ms, frame p50 ≤ off + 15% |

Under load the presses and depots cost 0.21 ms of server tick (about 0.8 µs per machine per tick) and 0.05 ms of
frame time (256 heads glided per tick, 256 stacks, 256 turning shafts); the check in each run reads one depot after the
flight (64 iron → 57 iron + 7 sheets). A first set of three runs had one run at 231 fps with 33 ms p99 frames
(`benchmarks/create-p3-processing-first.json`); five more runs did not repeat it, and DEBUG timings of the client's
depot messages (≤ 0.28 ms each, two or three depots a message) and press ticks (never over 1 ms) rule those out. Cause
unknown, watched for in later runs.

### Blocked / open

- Startup with Create's content installed: +0.5 s to the title screen and +0.9 s into a world (warm). The native mod
  does nothing at startup but scan the registry once; the time is Horizon reading Create's content: 25,349 more block
  states (56 walls are 18,144 of them), 5,557 asset/data files and Create's textures in the atlas. Not yet
  "no measurable cost": to investigate (fewer states via gap 2 own states for walls, or a faster content load in Horizon).

- Horizon pin: `feat/create-hooks` at `aea7ed878` (NATIVE_HOOKS_API 47). History: first `0a96593cf`, two commits on 9d979ca: gap 2
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
- Gap 1 landed as three Horizon commits: `91ea0acdc` (content `render_shape`), `a0fccb2df` (renderer:
  `gfx/parts.rs`, `entity.wgsl` `vs_part`: one instanced draw per model, instances uploaded only when their model's
  revision changes, angle `phase + rpm·6·seconds` on the GPU from the synced game time), `a81b8ef53` (API 42 helpers
  `native_part_model/set/clear`, light sampled at set and refreshed when a section remeshes). Create's partial models
  (`AllPartialModels`) are hidden content blocks (`part_*`, invisible, no item) with our own blockstates, so Horizon's
  model baker handles them; `shaft_half` is the first.
- Gap 8 is complete: API 41 (`chunk_load`/`chunk_unload`, `0a96593cf`) and API 43 (`chunk_data_save`,
  `native_chunk_data`, `native_mark_chunk_unsaved`, `d607feed5`). Mod state is typed in memory; NBT is written only
  when the chunk saves.
- Gap 2, custom models: Horizon reads `neoforge:obj` models (`9e557015d`: `horizon-assets/src/obj.rs`, mesh beside
  cuboid elements, baked as quads turned by the blockstate variant). 43 of Create's models are OBJ (wheels, flywheel,
  blaze burner, chain conveyor, roller, valve handle, tracks, bogeys). Not yet: `neoforge:composite` (7 models),
  connected textures (casings, windows), Create's material swap on water wheels (they draw with the OBJ's default
  textures).
- Parts are lit by the brightest of their block and the six beside it (the light inside an opaque block is none);
  Flywheel lights per vertex. A neighbour's light change in another section does not relight the part yet.
- Horizon fixes found by the port: recipe helpers (API 36) gave nothing in a server tick hook (no player swapped in;
  `b133ffc29`), and recipes in subfolders of `data/<ns>/recipe/` never loaded (`3f2df45b1`): Create keeps nearly all
  its recipes in subfolders, so its crafting and processing recipes were missing until then.
- The millstone takes any item through a hopper (Create only takes what it can mill): Horizon's mod containers have no
  per-item insert check yet (API 46 added the whole-container `only_when_empty` the depot needs).
- Gap 6, in-world items: API 45 (`cf41e2a18`) gives part instances an offset and a velocity (the GPU moves them from a
  start time, so a gliding item is uploaded once per change of motion) and bakes an item model into a part model
  (`ItemDisplayContext.FIXED`); API 47 (`aea7ed878`) says whether an item's model is lit like a block (`isGui3d`).
  Fake players (deployer) are still open.
- Gap 4, capabilities: API 46 (`b8392c26c`) adds `contents_changed` (NeoForge `ItemStackHandler.onContentsChanged`,
  fired when a hopper or dropper changes a mod container) and `ModContainer::only_when_empty`. Fluid handlers and
  per-item insert checks are still open.
- Horizon fix found by the belt: content blocks' declared `shapes` now also give their occlusion shapes
  (`95932161e`); before, a belt (an iron block's template) hid the top of the floor under it and the terrain below
  showed through. Small dark squares at the belt's edges in a top-down view are not explained yet.
- Depot and part keys: kinetic parts key by position and a 3-bit slot; a depot's up to 36 item parts (9 stacks × 4
  copies) key by a 64-bit mix of position, slot and copy with the top bit set.
- Multiplayer: a client gets depots and speeds when it joins and when they change; a second player walking into chunks
  another player already keeps loaded does not get them yet (one `sync` per join).
- Mod messages are limited to 32 per tick per player (about 1,600 part updates each); a very large network resolving
  at once would exceed it. Not handled yet (queue the rest for the next ticks).
- Horizon's `fill` drops block entity data for mod blocks (`setblock` and `data merge` keep it); vanilla applies it to
  every block filled. `/item` reaches only vanilla containers (Create's depot is no `Container` in Create either).
- A depot that joins holding a stack (a saved world, a block placed with its items) now gets its held stack at rest and
  asks its press: before, a reloaded depot under a press was never pressed.
- NeoForge common tags: Create's recipes name 53 `c:` tags (`c:ingots/iron`, `c:dusts/redstone`, every `c:dyes/*`...)
  that NeoForge defines, not Create, and vanilla has none: every such recipe found no ingredient (the iron sheet, half
  of Create's crafting). `create-data prepare` now writes those tags (60 with the ones they nest) from our table of
  NeoForge 21.1's memberships (`tools/create-data/src/common_tags.rs`). Found by a DEBUG log in the press's tick
  (`fits false` for iron). `c:milk` (a fluid tag) waits for fluids.
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
