# Porting Create 6.0.11 to Horizon (Nightly 9d979ca, MC 26.3)

Source of truth for behaviour: this repo's Java tree (`src/`, branch `main`, NeoForge 1.21.1); the port lives in `horizon/` on branch `port/horizon`.
Target: a horizon-mixins native bundle in `horizon/`, plus new hook points upstreamed to Horizon on
`/workspace/horizon` branch `feat/create-hooks` (based on `9d979ca`, `NATIVE_HOOKS_API` 40).

## Status (updated with every commit)

| Phase | State | Verified | Measured |
|---|---|---|---|
| 0 Setup | done: bundle `create` on 9d979ca, `create-engine` crate, glue module, `create-data compile/prepare`, MIT notice, perf baseline | stage/check/test/build green; fixture test; headless launch on BMG G21: content loads, mod idles without it | see Budgets |
| 1 Static content | in progress: 400 of 643 blocks and 35 plain items declared from exact vanilla templates; 1369 recipes, loot, tags, models, textures, lang from the prepare step | headless screenshot: casings, metal blocks, stone palettes, windows, shingles, stairs, slab, ladder, pillar, wall, scaffolding render with Create's textures | idle cost: see Budgets |
| 2 Kinetic core | in progress: solver (`kinetics.rs`: Create's ratios for shafts, gearbox faces, small/large cogs, split shafts; overpowering, direction conflicts, 256 RPM limit, stress/capacity from CStress), block→part table (`parts.rs`), per-world model (`world.rs`), glue on `block_changed` (hot: two range tests), `chunk_load`/`chunk_unload`, `level_tick_post` (solve only when dirty), `/create_debug speed`; turning parts (gap 1): the server sends changed speeds after a solve (`create:kinetics` mod messages, `client.rs` wire format; a joining client asks for all with `sync`), the client shows Create's models as GPU-turned instances (`client.rs`: own model for shafts/cogs, shaft halves for gearbox faces, motor and split shafts; Create's 22.5°/11.25° offsets); shaft, cogwheels, gearbox, encased shafts/cogs, clutch, gearshift, creative motor, hand crank declared. motor speed by mouse wheel on the motor (Create 5's scroll; Create 6's value board needs the overlay gap), saved with its chunk through API 43 (`chunk_data_save`, `horizon:mod_data`) and read back on load — proven: scrolled 16→32 rpm, saved, quit, relaunched, 32 rpm. clutch/gearshift follow redstone (`neighbor_changed`, hot: one range test; power applied after the level tick, before its solve: at most one tick later than Create) — proven: clutch powered 0 rpm / unpowered 16, gearshift powered −16. hand crank: right-click turns it 10 ticks at 32 rpm (sneak reverses), Create's food exhaustion, handle and shaft half drawn as turning parts (handle partial `part_hand_crank_handle`) — proven: 0 → −32 rpm on use, 0 again after 10 ticks; screenshots of the handle turning. water wheel: Create's flow score from the four blocks beside it (`waterwheel.rs`, after `WaterWheelBlockEntity` and Catnip's `VecHelper.rotate`), re-checked when a neighbour changes and every 60 ticks staggered by position, through API 44 `native_fluid_flow` (bubble columns as Create reads them); its OBJ wheel turns as a part (`part_water_wheel`) — proven: dry 0, water falling past the north side −8 rpm, both sides 0; screenshots of the wheel turning. **Not yet:** windmill, chain drives, belts, goggles' overstress display. Placement (`place.rs`, `native_place_block`, API 14): Create's directional blocks placed from an item take Create's facing (`DirectionalKineticBlock`, `CreativeMotorBlock`, `EncasedFanBlock`, `HandCrankBlock` on the clicked face recorded by `native_use_item_on`, `MechanicalPressBlock`/`HorizontalKineticBlock`, `SailBlock`): toward or away from the one shaft beside them (`getPreferredFacing`, none when shafts lie on two axes), else from the player's view (vanilla's `Direction.orderedByNearest` with 26.3's `Mth` sine table), sneaking flipping it; headless: looking north, a fan faces south, sneaking north, east of an x shaft east (its back to it); a motor south of a z shaft faces north and turns it (-16 rpm); a bearing and a press face south. **Not yet:** a fan's chute rule (no chutes). | engine 26 tests; fixture: chunk scan + hot hook + solve + server→client message → part instance at 16 rpm; headless: motor 16 → shafts 16 → gearbox +16 → cogs 16/−16 → large cog 8; unload/reload; opposing motors break the newer; two screenshots 0.7 s apart show the parts turning | see Budgets |
| 3 Processing | in progress: `processing.rs` (Create's processing recipes typed and indexed by input item, read once per world through API 36; `ProcessingOutput` rolls; `insertItemStacked`); millstone: small cog with a shaft below (impact 4), barrel-template block entity with 10 slots (hoppers in to slot 0, out of 1–9), Create's `MillstoneBlockEntity.tick` only for turning millstones, empty-hand use takes the products, turning stone drawn as a part. depot (`depot.rs`, after `DepotBehaviour`, `SharedDepotBlockMethods`, `DepotRenderer`): its stacks are the block entity's 9 slots (held 0, outputs 1–8; hoppers fill it only while it holds nothing, through API 46 `only_when_empty`, and take from every slot), the held stack's motion is typed state on the server; a right-click on its top gives back everything and puts down the hand's stack, sliding in from the player's side (`tick` only while it slides: an idle depot costs nothing); clients get its contents on `create:depot` (on change, on load, on join) and draw each stack as GPU item parts (API 45 `native_part_item_model`/`native_part_glide`, API 47 `native_item_uses_block_light`): Create's poses (flat items laid down, block items standing, a copy per two doublings, outputs 0.35 out at 45° steps with `new Random(i + 1)` angles), the slide run on the client tick by tick and glided between ticks on the GPU. mechanical press (`press.rs`, after `PressingBehaviour`, `BeltPressingCallbacks`): a shaft along its facing (impact 8); a depot two below asks it when its stack arrives or changes and when the press finishes or starts turning (an idle press or depot ticks nothing); the 240-tick cycle at Create's `getRunningTickSpeed`, the depot's stack locked while held, one item pressed at the head's lowest point (the whole stack when it is one: the first result becomes the held stack), results stacked into the outputs, overflow dropped above the depot; clients get each cycle on `create:press` (start, head down, done) and run it on, the head (partial `part_press_head`) moving on Create's curve, glided between ticks. Recipes from `create:pressing` (reloaded when `/reload` builds a new table). **Not yet:** pressing items lying in the world (Create's entity scan) and in basins (compacting), sequenced assembly, particles and sounds. encased fan (`fan.rs`, after `AirCurrent`, `FanProcessing`, `AllFanProcessingTypes`, `EncasedFanRenderer`): a shaft half at its back (impact 2); only a turning fan ticks (it joins the running list on a speed change and leaves it when stopped); its current is rebuilt on a speed change and every 31 ticks (`fanBlockCheckRate`): reach 3–20 by speed / 256, push or pull, the flow limit through the blocks ahead (no collision or `create:fan_transparent` passes, a full block stops, a partial shape by `findMaxDepth`), the segments of washing (water, `fan_processing_catalysts/splashing`), haunting (soul fire, lit soul campfire), smoking (fire, lit campfire, smouldering blaze burner) and blasting (lava, a blaze burner from fading up) by Create's priorities and block properties; depots and belt segments in the current and the block below it (horizontal) are processed: a stack's `processedBy`/`processingTime` (150 × ⌈count / 16⌉ + 1 ticks, -1 when its type has no recipe for it), then Create's results (`create:splashing`, `create:haunting` rolled per item and merged; vanilla's smoking, smelting else blasting, `_manual_only` filtered, food a smoker cooks to the same burnt; blasting burns what has no recipe unless fire resistant), on a depot the first held and the rest in its outputs, on a belt in place of the stack; the propeller (`part_fan_propeller`) turns at 5× speed clamped to 80–1,280 rpm on the GPU. Vanilla's cooking recipes come from the game's own table (`Game::recipes().cooking`): `native_recipes_of_type` serves only mods' types. Item entities in the current (API 52 `native_items_in`, gap 6): caught by a search every 5 ticks, pushed along the current (`speed / 512 / (distance / reach)`, an eighth a tick), processed by the type at their distance from the fan's face (Catnip's `alignedDistanceToFace`), the first result replacing the stack and the rest dropped beside it, none discarding it. **Not yet (fan):** mobs and players pushed, burnt or extinguished (no API for other entities), an item's progress kept after it leaves every current (Create keeps it in the entity), particles, sounds, chutes, saving a belt stack's progress. crushing wheels (`crusher.rs`, after `CrushingWheelBlock`, `CrushingWheelControllerBlock`, `CrushingWheelControllerBlockEntity`): a wheel is a shaft on its axis (impact 8) whose whole OBJ model turns as a GPU part; when a wheel is placed, broken or changes speed, the controllers on its four sides are fixed (`updateControllers`: one between two wheels on the same axis two apart, valid when they turn against each other, facing the wheel's axis crossed with the side, signed by its turn; an invisible, shapeless block of its own, `structure_void`'s template, placed with `native_set_block`); a valid controller crushes at the wheel's speed / 50 and only turning ones tick: the first item entity in its cell is held (drawn to the middle, sinking at up to half a block a tick), taken in a quarter past the middle (API 52), its recipe (`create:crushing`, else `create:milling`) run down at 4 × speed, slower by log2 of the count until applied at under 20, each item rolled and merged into up to 31 stacks (none: the stack is destroyed), then thrown out past the facing side; an item at or past that side is not caught again (`BypassCrushingWheel`); contents saved with the column (`crushers`). **Not yet (crushing):** handing results to a belt or depot below, mobs and players crushed, the wheels' push on items on top of them, particles and sounds; the wheel's full-cube declared shape also hides neighbouring faces (Create's wheel doesn't occlude). blaze burner (`burner.rs`, after `BlazeBurnerBlockEntity`, `BlazeBurnerBlock`): its fuel is typed state (none, normal, special, time left), saved with the column (`burners`); only a burning burner ticks; `tryUpdateFuel` from a right-click (a furnace fuel's burn time from the game's providers, the regular tag at 1,600, a blaze cake or the special tag superheating at 3,200; a player tops normal fuel up to 10,000; one used up outside creative, a lava bucket leaves its bucket), the heat (smouldering, kindled, fading under 1.25%, seething; special fuel ends in 5,000 of normal) written to the `blaze` property that fans read; flint and steel lights an empty burner into a lit burner. The cage and, by heat and facing, the blaze head are one multipart blockstate of ours (the head at rest is in the chunk mesh: no part, no upload). **Not yet (burner):** the head bobbing and turning to players, rods, flame, light level by heat (Horizon's `light` is per block), the empty-burner item catching a blaze, creative cake, goggles and hats, hoppers and funnels putting fuel in. basin and mechanical mixer (`basin.rs`, after `BasinBlockEntity`, `BasinRecipe`, `BasinOperatingBlockEntity`, `MechanicalMixerBlockEntity`, `HeatCondition`): the basin's block entity holds 9 inputs and 9 outputs (hoppers put into the inputs, take from the outputs; items landing in it go into the inputs, API 49); the mixer is a shaftless small cog (impact 4) and looks at the basin two below only when asked (`basinChecker`: placed, its speed changed, the basin's contents changed, a cycle ended), then runs Create's 40-tick cycle with `log2(512 / speed) × ⌈time / 100 × 15⌉ + 1` ticks at the bottom, applying the recipe with the most ingredients that the basin holds and whose heat the block below gives (a blaze burner's level, smouldering for other passive heaters), one item per ingredient, results rolled into the outputs only if all fit, and staying down while the recipe still fits; only running mixers tick. Recipes: `create:mixing` with item ingredients and results (fluid ones are left out until gap 5). The cog turns with the mixer, the pole and whisk hang at rest, the whisk turning at half speed. Compacting: a turning press with a basin two below is asked as the mixer is and runs its cycle in Create's basin mode (the head reaching 22/16 down), applying the `create:compacting` recipe at the bottom and looking again when done. **Not yet (basin):** fluids (gap 5), shapeless crafting in the mixer and crafting recipes the press compresses (`canCompress`), the basin's facing outputs and filter, the head's stroke and faster spin while mixing, particles and sounds. **Not yet:** saving the millstone's timer and the depot's angle (an angle is 0 after a reload), items falling onto them, Catnip's `nudge`, upright items facing the camera, sounds, the other machines | engine 42 tests; fixture: depot put down → one `create:depot` message → the client glides the stack in from the south; headless: hopper of wheat above a millstone on a 16 rpm motor: 2 wheat milled in 18 s (150 ticks each), flour and seeds in its slots, taken by hand; depot: 16 iron put down by hand (block data `Items: [{count: 16, Slot: 0b}]`, hand empty), drawn as three flat copies; a hopper above a second depot moved one bread in and no more (the hopper kept 2 bread, 2 apples); an empty hand took the 16 iron back; press: creative motor → press at 16 rpm over a depot holding 5 iron: 4 iron + 1 sheet after 3 s, 3 + 2 after 13 s (one per 6 s cycle: 240 cycle ticks at 2 a tick), the head drawn lower mid-stroke; fixture: motor → press over a depot, one of five ingots pressed in 80 ticks; fan: engine tests for reach, segments, flow limit, timer and each type's recipes; fixture: motor → fan → water → depot of 2 ice, still ice after 100 ticks, packed ice after 160; headless (BMG G21): five fans at 16 rpm blowing south: water over a depot of 3 ice → 3 packed ice, a lit campfire over 2 beef → 2 cooked beef, lava over 40 sand → 40 glass (451 ticks) and over 5 dirt → gone, a soul campfire over 2 potatoes → 2 poisonous potatoes, water over a still belt with 3 red wool → 3 white wool; screenshots show the propeller turning behind the grille, horizontal and facing up; dropped items: 2 ice and 1 sand thrown into a fan's water current were blown to the glass at its end and became 2 packed ice and 1 clay ball; crushing: two z-axis wheels driven at -16 and 16 rpm get a valid controller facing down between them; 4 gravel dropped into the gap were taken in and, after the timer (~19 s), 4 sand, 1 clay ball and 1 flint lay below; a screenshot shows both wheel models turned; burner: coal on a smouldering burner → `blaze=kindled`, flint and steel on an empty one → `lit_blaze_burner`, a fan blowing through a kindled burner blasted 2 raw iron on a depot into 2 iron ingots; a screenshot shows the blaze head facing south in its cage and the lit burner's fire; basin: two mixers driven through cogs at 16 rpm: 2 andesite + 2 iron nuggets dropped into an unheated basin → 2 andesite alloy in its outputs, copper and zinc ingots in a basin on a kindled burner → 2 brass ingots; a screenshot shows the mixers' poles and whisks hanging into the basins; compacting: a press at 16 rpm over a basin of 18 snow blocks → 2 ice, the head drawn reaching into the basin | pending |
| 4 Logistics | in progress: mechanical belt (`belt.rs`, after `BeltBlock`, `BeltBlockEntity`, `BeltHelper`, `BeltInventory`, `BeltRenderer`): chains found from block states as Create's `initBelt` walks them (rebuilt when a segment is placed or its column loads), segments of one belt turn together (`propagateRotationTo`: same controller, ratio 1), pulleys carry the shaft; the items are a plain list on the controller, ticked only while the belt holds items (`BeltInventory.tick`: a block of spacing, Create's endings — into a depot or another belt, blocked by a sturdy face, thrown off — and presses over a segment holding and pressing items); items arrive as dropped items landing on it (API 49 `update_entity_after_fall_on`, also into empty depots), an empty hand takes the items of a segment; breaking a segment drops the belt's items and turns the next segments into shafts or air, chain by chain (`onRemove`). Clients get a belt's items on `create:belt` when they change and run the same tick; each item is a GPU item part gliding at the belt's speed (flat items turned by their angle on the GPU, sloped ones baked with their tilt), uploaded again only when its motion changes; segments draw Create's belt partials posed by its renderer's turns (API 48 `native_part_model_posed`), the pulleys turning. **Not yet:** the belt connector item (belts are placed by commands), the belt texture's scrolling, casings, tunnels, funnels, chutes, entities riding belts. Items on a belt are saved with its controller's column (`chunk_data_save`, as `BeltInventory.write`) and back on it when its chain is whole again | engine 54 tests; fixture: a motor turns a 3-long belt, 4 gold ingots landing on its start ride it onto the depot past its end; headless: creative motor → 6-long belt south at 16 rpm (items 0.0333 blocks a tick: 16/480) → depot: 5 iron and 2 oak logs dropped on the start, the iron ends on the depot (block data `count: 5`), the logs wait at the end while the depot is full, no item entity left; screenshots of the stacks riding the belt, a top-down shot aligned between reference blocks; saved: iron and gold dropped on a stopped belt, quit, relaunched: both back on the belt, a motor added, the gold (further along) reaches the depot first (3 runs) | pending |
| 7 Redstone | in progress: powered latch and powered toggle latch (`latch.rs`, after `PoweredLatchBlock`, `ToggleLatchBlock` and vanilla's `DiodeBlock`): built on the repeater (shape, placement, survival), with their own states; their rules through the hooks: inputs looked at when a neighbour changes, the scheduled tick of delay 1 one game tick later (the glue keeps it: the repeater's own tick is skipped through `tick_block`), the output answered from the latch's state (`block_signal`, `is_signal_source`), right-click flips the output (a powered latch only while back and sides agree); reading the back and sides through Horizon's `Behaviours::signal` (`0ac3e823f`, `getSignal` toward one direction) and redstone dust's power. Tick priorities are vanilla's normal, not Create's HIGH/VERY_HIGH. **Not yet:** redstone links, pulse repeater/extender, analog lever, nixie tubes, display links, contacts, observers, threshold switch | engine 57 tests; fixture: a signal at a toggle latch's back turns it on a tick later, its south face gives 15 and its north 0, the repeater's tick is handled; headless: toggle latch with a lever behind and a lamp in front: lamp off, on with the lever, still on when the lever goes off, off at the next press; powered latch: set by its back lever, held, reset by its side lever | pending |
| 6 Equipment | in progress: goggles (gap 7, through the existing `hud` hook, `native_hud`): the item (stacks to 1, `equippable` on the head); worn, the kinetic block under the crosshair shows Create's goggle tooltip right of the crosshair (`GoggleOverlayRenderer` at width / 2 + 20, vanilla's tooltip colours): "Kinetic Stats:" with its stress impact × current speed, "Generator Stats:" with its capacity × current speed (`addToGoggleTooltip`, `LangNumberFormat`'s grouping); the hot path is one compare on the head slot per frame. Proven: fixture (no goggles, nothing; goggles on a motor, "Generator Stats:"); headless: a press at 16 rpm shows "128su at current speed", the creative motor "262,144su at current speed". **Not yet (goggles):** the network's total stress and capacity (the server solves it; clients don't get it yet), other blocks' goggle info (tanks, burners), the fade-in, the goggles drawn on the head. wrench (`wrench.rs`, after `WrenchItem` and `IWrenchable`): the item (stick template, stacks to 1); used on a Create block it turns it about the axis of the face clicked (`getRotatedBlockState`: `axis`, horizontal `facing` from the top or bottom, any `facing` turned until off the clicked axis, vanilla's `getClockWise(axis)`), sneaking it picks the block up (its item to the player unless creative, the block gone without drops); latches let it through as Create's do; belts are left for their own wrench use (`BeltSlicer`). **Not yet:** wrench on non-Create blocks (`create:wrench_pickup` tag), the wrench's own model, goggles (gap 7), other equipment | engine 59 tests (vanilla's three quarter turns checked as one rule, `d × axis`); fixture: a wrench on a shaft's top turns x to z, a stick is not taken; headless: shaft along x clicked on top → axis z; a creative motor sneak-wrenched → gone, no item added in creative | — |
| 5 Fluids | in progress: pipes and the mechanical pump (`fluids.rs`, after `FluidPipeBlock`, `PumpBlockEntity`, `FluidPropagator`, `FluidNetwork`, `OpenEndedPipe`): a pipe's six sides are its own states, set by the glue when it or a neighbour changes (`updateBlockState`: toward pipes, tanks and pumps along their axis; one connection gets an open end opposite; alone, straight toward the player's view); the pump is a small cog on its facing's axis (impact 4) pushing out of its facing and pulling from its back (Create 6: speed doesn't reverse it), its cog drawn turning; a turning pump finds the ends of its pipes only when a pipe, pump or tank changed (breadth first, `mechanicalPumpRange` 16; pressure = speed, shared among ends at the same distance, a simplification of `validFaces`), then moves `max(1, pressure / 2)` mB a tick from the first end behind it (a tank, or an open end's source block taken a bucket at a time as `removeFluidFromSpace` does, leaving level 14 for vanilla to refill an infinite pool) to the first end in front that takes it (a tank, or an open end placing a source per bucket). **Not yet (pipes):** fluid travelling through pipes over time and drawn in them, several sources per network, fluid reactions, brackets, glass and encased pipes, valves, smart pipes, drains, spouts, hose pulleys, potions and Create's own fluids; pushing into an open end is written but not proven in game. fluid tank, one block (`tank.rs`, after `FluidTankBlock`, `FluidTankBlockEntity`, `FluidTankRenderer`): glass template (`noOcclusion`, windows), copper sounds, its own `top`/`bottom`/window `shape` states; 8 buckets of water or lava (`fluidTankCapacity`), typed in memory and saved with its chunk; creative players empty and fill buckets into it, whole buckets only (Create 6 leaves survival to spouts and drains), the bucket's own use after the click taken (`use_item`) as Create's SUCCESS stops it on the client; the fluid drawn as `FluidTankRenderer`'s box at its level, the fluid's still texture (water tinted `0x3F76E4`) through API 50 `native_part_model_tinted`. **Not yet:** multiblock tanks (`ConnectivityHandler`), connected textures, comparators, pipes, pumps, spouts, drains, Create's own fluids (gap 5), boilers | engine 61 tests; fixture: a creative water bucket fills a tank (saved as `[x, y, z, 0, 1000]`), the following bucket use is taken once, the client draws the water's box; headless: 4 water buckets into one tank, 2 lava into another, drawn at their levels, nothing placed in the world; saved, quit, relaunched: both still filled; pump: engine tests (pipe sides, ends and shared pressure); headless: a 3×3 water pool → 2 pipes → pump at 16 rpm (driven by a cog) → 2 pipes → tank: the pipes connected east–west, the tank gained 800 mB every 100 ticks (8 mB a tick, DEBUG log, removed) and the pool stayed 9 sources; a screenshot shows the pipes, pump, cog and the water in the tank's window | — |
| 8 Contraptions | in progress: mechanical piston (`piston.rs`, after `MechanicalPistonBlockEntity`, `LinearActuatorBlockEntity`, `PistonContraption`, `Contraption.searchMovedStructure`): piston, sticky piston, extension pole and head declared; the piston is a shaft across its facing (`axis_along_first`); a speed change makes a still piston assemble (`assembleNextTick`): the poles behind and in front (`collectExtensions`) set its range and progress, the structure in front is searched from the anchor (slime and honey stick, the block ahead in the direction of travel is pushed, an unmovable one ahead refuses), all of it leaves the world (the piston `moving`), moves `clamp(speed / 512, ±0.49)` blocks a tick and goes back at the end (`extended` or `retracted`); a stop puts it back at the nearest whole offset; clients get the structure on `create:piston` and draw it as one compound part gliding on the GPU, uploaded only when it starts, changes speed or stops. **Not yet (piston):** collisions with the world and entities on the way, carried block entities, placement from an item (`DirectionalAxisKineticBlock`), Create's casing shapes, gantries, chassis, glue. windmill bearing and sails (`contraption.rs`, after `WindmillBearingBlockEntity`, `BearingContraption`, `Contraption.moveBlock`, `BlockMovementChecksImpl`): an empty hand assembles the sails on the next tick (`assembleNextTick`) or takes them apart at once; the structure is the block in front of the bearing and what holds on to it (sails to the sails beside them in their plane, slime and honey), at most 2,048 blocks, at least 8 sails (`minimumWindmillSails`), the bearing itself left out; its blocks leave the world and the bearing generates `clamp(sails / 8, 1, 16)` rpm (`windmillSailsPerRPM`, clockwise), capacity 512; taken apart (at angle 0, as a windmill turns back first) its blocks go back where they were, or drop. The structure is one GPU part (Horizon API 51 `native_part_model_compound`: every block baked into one model) turning about the bearing's axis at its speed, the bearing's wooden top and shaft half turning with it; saved with the chunk (state names) and reassembled on load. Blocks with block entities are refused (moving them isn't ported; Create moves them). **Not yet:** contraption entities (gap 3: collision, riding), mechanical bearings driven by the network, pistons, gantries, pulleys, carts, super glue, chassis, the direction setting (value box, gap 7) | engine 63 tests; fixture: eight sails assemble the next tick (gone from the world, 1 rpm), saved, the off hand's click taken, a second click puts them back; headless: 17 white sails in a cross in front of a bearing: assembled, the bearing and the shaft behind it at 2 rpm, screenshots of the sails turning; saved, quit, relaunched: 2 rpm, drawn turning; clicked again: the 17 sails back, 0 rpm (2 runs, all steps); piston: engine tests (poles, structure, speed); headless: a piston facing east with three poles behind it, driven at -16 rpm, pushed a stone and a gold block: two poles and the head in front, stone and gold three blocks further, the piston `extended`; a screenshot mid-move shows the structure gliding | — |
| 9 Trains | in progress: first slice (`trains.rs`, after `Train`, `Navigation`, `StationBlockEntity`): the station and small/large bogeys declared; a station's track is the straight track beside it (`xo`/`zo`, traced both ways into a line); using the station assembles the bogeys standing on its line (a block above the track) and what sticks to them (slime, honey: super glue isn't ported) into a train, lifted out of the world; a train sent to a station on its line runs there with Create's navigation (28 m/s top speed, 3 m/s² acceleration, braking distance `v² / 2a`, the slow approach over the last ten blocks, a quarter block's overshoot) and stops within half a block; using the station it stands at puts it back in the world. Clients draw each train as one compound part gliding along the track, sent when it starts, every ten ticks while its speed changes, and when it stops. The destination is given by `/create_debug train <station>` (and assembly by `/create_debug assemble <station>` in the benchmark): test drivers standing in for schedules and the station's screen. **Not yet (trains):** curves, slopes, junctions and the track graph, signals, schedules and the train controls, carriages and their couplings, bogey wheels drawn, passengers, saving trains, station names and the station's own target placement. train track (`TrackBlock`): its own `shape` (Create's `TrackShape`, ZO first as its default), `turn` and `waterlogged` states, drawn by Create's track models; collision as Create's (2 px, none on a slope), used for the outline too. **Not yet:** the track graph (`TrackGraph`, `TrackPropagator`), curves (`turn`, bezier connections, the track item's placement), bogeys, stations, carriages (gap 3), schedules, signals, the train HUD | headless: straight tracks along z and x, a crossing, diagonals and a slope placed: drawn with Create's models (screenshot); 24 tracks counted; train: engine tests (a line, a run to 400 blocks reaching 28 m/s and stopping at the station); headless: a 61-block straight track, stations 50 apart, a small bogey with a slime and a gold block on it: assembled at the first station (all three left the world), sent to the second, put back there intact (bogey, slime, gold at the second station); a screenshot mid-run shows the train on the track | — |

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

| Scenario (2026-11-01, Phase 3 fans, 25 s settle, same session) | fps | frame p50 / p99 (ms) | server tick (ms) | Budget |
|---|---|---|---|---|
| off: no content mod | 1844 ± 3 | 0.498 / 1.132 | 4.258 ± 0.126 | baseline |
| fans: 32 fans at 16 rpm through water over 3 depots of 64 ice each (160 Create blocks) | 1825 ± 153 | 0.519 / 1.051 | 4.501 ± 0.170 | tick ≤ off + 0.5 ms, frame p50 ≤ off + 15% |

32 running fans with their currents over 96 depots cost 0.24 ms of server tick and 4% of frame p50
(`benchmarks/create-p3-fans-*.json`; a first measurement before fans looked for item entities gave +0.09 ms). Each run reads one row's three depots after the flight: 64 packed ice each.
A stack of 64 takes 601 ticks, so most washing ends near the start of the flight; the fans keep ticking their currents
and depots throughout.

| Scenario (2026-11-01, Phase 3 crushing, 25 s settle, same session) | fps | frame p50 / p99 (ms) | server tick (ms) | Budget |
|---|---|---|---|---|
| off: no content mod | 1784 ± 76 | 0.530 / 1.109 | 4.460 ± 0.057 | baseline |
| crushers: 30 pairs of wheels at 16 rpm, each fed 16 gravel (120 Create blocks + 30 controllers) | 1762 ± 23 | 0.520 / 1.113 | 4.459 ± 0.097 | tick ≤ off + 0.5 ms, frame p50 ≤ off + 15% |

Thirty crushing controllers working and sixty turning wheel models cost no measurable tick or frame time
(`benchmarks/create-p3-crushers-*.json`); each run finds sand, flint and clay balls lying under the gaps.

| Scenario (2026-11-01, Phase 3 burners, 25 s settle, same session as its off run) | fps | frame p50 / p99 (ms) | server tick (ms) | Budget |
|---|---|---|---|---|
| off: no content mod | 1811 ± 5 | 0.511 / 1.175 | 4.330 ± 0.111 | baseline |
| burners: a 32×32 floor of smouldering blaze burners (1,024 passive) | 1881 ± 66 | 0.501 / 1.081 | 4.404 ± 0.027 | tick ≤ off + 0.25 ms, frame p50 ≤ off + 10% |

Passive burners tick nothing (+0.07 ms, inside the off run's spread) and their heads, part of the chunk mesh, cost no
frame time (`benchmarks/create-p3-burners-*.json`).

| Scenario (2026-11-01, Phase 3 mixers, 25 s settle, same session) | fps | frame p50 / p99 (ms) | server tick (ms) | Budget |
|---|---|---|---|---|
| off: no content mod | 1881 ± 5 | 0.492 / 1.160 | 4.402 ± 0.086 | baseline |
| mixers: 32 basins of 64 andesite + 64 nuggets under mixers at 16 rpm (128 Create blocks) | 1846 ± 91 | 0.512 / 1.116 | 4.355 ± 0.118 | tick ≤ off + 0.5 ms, frame p50 ≤ off + 15% |

Thirty-two mixers mixing cost no measurable tick and 4% of frame p50 (96 turning parts)
(`benchmarks/create-p3-mixers-*.json`); each run's check reads one basin mid-way (53 + 53 left, 11 alloy made).

| Scenario (2026-11-01, Phase 5 pumps, 25 s settle, same session) | fps | frame p50 / p99 (ms) | server tick (ms) | Budget |
|---|---|---|---|---|
| off: no content mod | 1796 ± 48 | 0.529 / 1.107 | 4.431 ± 0.107 | baseline |
| pumps: 10 pumps at 16 rpm pulling from pools through pipes into tanks (80 Create blocks) | 1853 ± 73 | 0.510 / 1.029 | 4.380 ± 0.127 | tick ≤ off + 0.25 ms, frame p50 ≤ off + 10% |

Running pumps cost no measurable tick or frame time: their ends are found only when pipes change, and each tick moves
one amount between two ends (`benchmarks/create-p5-pumps-*.json`). The scenario has no in-run check of the tanks
(their contents are not readable by commands); the headless proof above shows the same layout filling a tank.

| Scenario (2026-11-01, Phase 9 trains, 25 s settle, same session) | fps | frame p50 / p99 (ms) | server tick (ms) | Budget |
|---|---|---|---|---|
| off: no content mod | 1822 ± 54 | 0.509 / 1.102 | 4.516 ± 0.143 | baseline |
| trains: 12 straight tracks of 57 (684 track blocks, 24 stations), 12 trains sent across as the flight starts | 1758 ± 46 | 0.543 / 1.043 | 4.660 ± 0.028 | tick ≤ off + 0.5 ms, frame p50 ≤ off + 15% |

Twelve trains running cost 0.14 ms of tick and 7% of frame p50 with their 684 track blocks in view
(`benchmarks/create-p9-trains-*.json`); each run's check reads the twelve replies ("train N heading for ...").

Under load the presses and depots cost 0.21 ms of server tick (about 0.8 µs per machine per tick) and 0.05 ms of
frame time (256 heads glided per tick, 256 stacks, 256 turning shafts); the check in each run reads one depot after the
flight (64 iron → 57 iron + 7 sheets). A first set of three runs had one run at 231 fps with 33 ms p99 frames
(`benchmarks/create-p3-processing-first.json`); five more runs did not repeat it, and DEBUG timings of the client's
depot messages (≤ 0.28 ms each, two or three depots a message) and press ticks (never over 1 ms) rule those out. Cause
unknown, watched for in later runs.

| Scenario (2026-10-31, Phase 4, 25 s settle, same session, idle frame cap off) | fps | frame p50 / p99 (ms) | server tick (ms) | Budget |
|---|---|---|---|---|
| off: no content mod | 1891 ± 76 | 0.516 / 1.034 | 4.212 ± 0.207 | baseline |
| logistics: 32 belts × 48 at 16 rpm, 32 motors, 672 stacks of 8 iron dropped on them, stone past each end | 1621 ± 21 | 0.604 / 0.998 | 4.132 ± 0.083 | tick ≤ off + 0.5 ms, frame p50 ≤ off + 20% |

Belts holding items cost no measurable server tick; 1,568 belt segments (top, bottom and the pulleys turning: about
3,100 part instances) and the stacks on them cost 0.09 ms of frame time. Each run checks no stack is left lying as an
entity. Before measuring this, the client looked every item's model up again every tick (630 fps); now an item is
looked at only when its motion changes. `bench_create.py` sets `inactivityFpsLimit:minimized`: vanilla's default caps
frames at 30 a minute after the last input, which a long scene build reaches (a first logistics run measured exactly
30 fps).

| Scenario (2026-10-31, Phase 7, 25 s settle, same session) | fps | frame p50 / p99 (ms) | server tick (ms) | Budget |
|---|---|---|---|---|
| repeaters: 32 observer clocks into chains of 16 vanilla repeaters (control) | 1937 ± 59 | 0.502 / 0.937 | 4.665 ± 0.039 | control |
| redstone: the same clocks into chains of 16 toggle latches (512 latches, ripple counters) | 1953 ± 86 | 0.497 / 0.909 | 4.866 ± 0.033 | tick ≤ control + 0.3 ms, frame p50 ≤ control + 10% |

Each run reads one chain twice a second apart: the latches count (`0000111011111111` then `1011011011111111`, the
first bit nearest the clock), the repeaters pass the pulses on. The latches cost 0.2 ms more than repeaters: their
glue asks every neighbour of a changed latch to read it again (`native_update_signal_neighbors`), where a diode
updates only the block in front.

| Scenario (2026-10-31, Phase 8, 25 s settle, same session) | fps | frame p50 / p99 (ms) | server tick (ms) | Budget |
|---|---|---|---|---|
| off: no content mod | 1902 ± 45 | 0.510 / 1.022 | 4.121 ± 0.067 | baseline |
| contraptions: 16 windmills of 17 sails assembled and turning at 2 rpm | 1863 ± 78 | 0.524 / 0.953 | 3.972 ± 0.071 | tick ≤ off + 0.25 ms, frame p50 ≤ off + 10% |

Each run checks that all 16 bearings turn before measuring. A turning windmill is one instanced draw of one model: no
measurable server tick, 0.014 ms of frame time for the 16.

Equipment (the wrench) and the fluid tank do no work per tick: their cost is the idle row above (content on, nothing
running).

### Blocked / open

- Startup with Create's content installed (warm, `perf_bench`, render distance 12, 3 runs): title screen 748 ± 9 ms
  without, 1161 ± 22 ms with Create (`benchmarks/perf/create-p0b-*`); first measured 854 / 1379 ms. The native mod does
  nothing at startup but scan the registry once: the time is Horizon reading Create's content, 25,349 more block states
  (56 walls are 18,144 of them: 250 ms of the difference, measured by leaving them out). DEBUG timings of the loading
  steps: block facts (`gameplay-facts.json`, 104 MB with Create against 56 MB: ~1.9 KB a state), the registry
  (`facts.json`), model states, occlusion and collision tables all grow with the states; the texture atlas adds about
  100 ms after them. Horizon fix `1754ad635`: those tables are read whole and parsed from memory (serde_json from a reader
  was several times slower) and the block facts' validation pass runs beside their decode instead of before it: the
  core tables went from 461 to 321 ms without Create and from 791 to 555 ms with it. **Open:** the rest is proportional
  to the size of Horizon's per-state JSON tables; making an unused content mod cost nothing at startup needs Horizon to
  cache those tables decoded (binary) or load them lazily. Ticks and frames with Create installed and unused are within
  noise of mods off (see the idle rows).

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
- Gap 6, in-world items: API 52 (`105f87c0d`) gives mods the item entities in a box (`native_items_in`: stack, position, motion, an emptied stack discarded). API 45 (`cf41e2a18`) gives part instances an offset and a velocity (the GPU moves them from a
  start time, so a gliding item is uploaded once per change of motion) and bakes an item model into a part model
  (`ItemDisplayContext.FIXED`); API 47 (`aea7ed878`) says whether an item's model is lit like a block (`isGui3d`).
  Fake players (deployer) are still open.
- Gap 4, capabilities: API 46 (`b8392c26c`) adds `contents_changed` (NeoForge `ItemStackHandler.onContentsChanged`,
  fired when a hopper or dropper changes a mod container) and `ModContainer::only_when_empty`. Fluid handlers and
  per-item insert checks are still open.
- Horizon fix found by the belt: content blocks' declared `shapes` now also give their occlusion shapes
  (`95932161e`); before, a belt (an iron block's template) hid the top of the floor under it and the terrain below
  showed through. Small dark squares at the belt's edges in a top-down view are not explained yet.
- The `block_signal` hook's `face` is the side of the block touching the reader; vanilla's `getSignal` direction points
  from the reader to the block, its opposite. The latches first answered on the wrong side (found by a DEBUG log of the
  signal asked of a lit latch).
- In the `use_item_on` hook, `native_acting_player()` gave none for the window's own player on the server thread; the
  wrench takes the level in place (`game.dim`) instead. Found by DEBUG logs. In one earlier run a sneaking click
  removed a motor without the hook logging it; not seen again, not explained.
- Horizon's client also sends the off hand's click after a main-hand click a Create block took (vanilla's never does:
  SUCCESS ends the use); a windmill assembled by the main hand was taken apart by the off hand a tick later. The glue
  takes that one off-hand click (the same player, within 10 ticks), as it takes the item use. Found by DEBUG logs.
- `mixins/create/runtime.rs` and the hook helpers are no crate's files, so `cargo fmt` never formatted them; two
  source-audit failures came from that (prettyplease formats an unformatted closure chain differently on both sides).
  `tools/horizon.py check` now runs `rustfmt --check` on them.
- A click a Create block takes on the server is followed by the client's item use (Horizon's client doesn't know the
  mod answered SUCCESS): a bucket used on a tank also emptied into the world. The glue now takes that one use
  (`use_item`, the same player within a tick), for every click its blocks take.
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
