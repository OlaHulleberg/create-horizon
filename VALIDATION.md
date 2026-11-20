# Validation

Checked on 2026-11-20 (Linux x86_64, Rust 1.99.1, a fresh build directory), pinned to Sunrise
`94256510f74e6843754e0a93fd9e4da7e45b1056` in `create/sunrise-mod.toml`.
`cargo sunrise-mod` uses the signature-verified release tool and SDK from
`sdk-94256510f74e6843754e0a93fd9e4da7e45b1056`, with the published release key.
No SDK mirror or trusted-key override is used. Mixins stays at
`e59da9094f0c2fdda6c627fe9eff22dec6a906f0`, as the release SDK specifies.

`684e89b` on `9425651` passed the in-game smoke test below. Earlier in-game results are historical: `6373106` ran on `520fc48`, the developer build was `b77574e`,
the first nightly smoke test used `2d81263`, and the registry smoke tests used `ed3ac19`.

## Checks

| Where | Command | Result |
|---|---|---|
| `create/` | `cargo +1.99.1 fmt --check` | Pass |
| `create/` | `cargo +1.99.1 check` and `cargo +1.99.1 clippy --all-targets -- -D warnings` | Pass |
| `create/` | `cargo +1.99.1 test` | Pass (101 glue tests) |
| `create/` | `cargo +1.99.1 test -p create-engine` | Pass (820 engine tests) |
| `create/` | `cargo sunrise-mod check` | Pass (official `94256510f7` SDK) |
| `create/` | `cargo sunrise-mod build` | Pass (native library and `.smod` package) |
| `create/` | `cargo sunrise-mod fits target/sunrise-mod/create.smod --build 94256510f74e6843754e0a93fd9e4da7e45b1056` | Pass (`made for`) |
| `create/src/engine/` | `cargo +1.99.1 fmt --check` | Pass |
| `create/src/engine/` | `cargo +1.99.1 check` and `cargo +1.99.1 clippy --all-targets -- -D warnings` | Pass |
| `tools/create-data/` | `cargo +1.99.1 fmt --check` | Pass |
| `tools/create-data/` | `cargo +1.99.1 check --locked` and `cargo +1.99.1 clippy --locked --all-targets -- -D warnings` | Pass |
| `tools/create-data/` | `cargo +1.99.1 test --locked` | Pass (53 tests, including `committed_assets` with `CREATE_JAR` set to the pinned jar) |
| `tools/create-data/` | `cargo run --locked -- registry --out ../../create --check` | Pass |

## Registry package smoke test on `ed3ac19`

From a clean `git archive` of the registry data commit, with the signed release SDK and no `create-data prepare`
outputs: `cargo sunrise-mod build`, `cargo sunrise-mod check` and `cargo sunrise-mod check` on the `.smod` passed.
The package carries `LICENSE` and `NOTICE.md`. The `[[jar]]` entry references the player's verified Create
1.21.1-6.0.10 jar; no jar is in the package. `cargo sunrise-mod run --resources <vanilla resources> -- <game args>`
under Xvfb and llvmpipe, without the launcher, staged the package's data and assets unchanged and opened a new
creative world. `/give @s create:cogwheel`, and the same command for `create:zinc_ingot` and `create:wrench`,
all succeeded. The cogwheel, motor and depot drew in the world, and the cogwheel and zinc ingot drew in the hotbar.
At that point the wrench occupied a slot but had no icon or held model. The visual test below verifies the
replacement item model and other assets migrated off local preparation.

## Registry visuals on `ed3ac19`

Built and checked a second `.smod` from a clean checkout with no `create-data prepare` outputs. The 5,916 staged
manifest and resource entries matched the `.smod` byte for byte. Ran it without a launcher with the player's
pinned jar under Xvfb and llvmpipe:

| Scene | Observed |
|---|---|
| Nixie tube, display link, stock link | The original composite parts drew, including the nixie glass and numerals. |
| Two minecarts linked using a coupling item | Attachments, rings and textured link drew. |
| Steam whistle on a tank over a lit campfire, powered by a redstone block | The whistle blew; successive steam frames drew using the packaged shader and still drew after a resource reload. |
| Two connected chain conveyors driven by a creative motor | The motor drove the conveyors at 16 rpm and the chain texture scrolled between frames. |
| Wrench, potato cannon, handheld worldshaper | Hotbar icons and held models drew. In fixed item frames the gear, cog and accelerator changed angle while the bodies stayed still. |

Local `prepare` still supplies connected textures, tinted models, model-derived placement ghosts and other
unmigrated assets. The package does not replace them; the checkerboard, glue and icon pipelines still log
missing sprites without local `prepare`.

## Declared shapes and piston heads on `ed3ac19`

In the registry smoke-test world, placed a torch and a standing sign on a metal girder, and a wall sign on the
full declared face of a mechanical piston head. The attachments stayed in place. A creative motor extended a
mechanical piston through two poles. The new head was present and `/data get block` reported no block entity;
the same command on a vanilla player head returned `minecraft:skull`. Reversing the motor retracted the piston
and removed its head.

## In game

A local developer base built from Sunrise at the pin, the mod built by the launcher's `native-build` from
`create-data prepare` of Create 1.21.1-6.0.10's jar, fresh creative worlds, driven by `horizon-ctl` (player clicks,
commands, screenshots read back, `/data get` and the mod's `create_debug` readouts).

### Features

| # | Item | Result | Evidence |
|---|---|---|---|
| 1 | Stock ticker category editor | Pass | Cards added (list and attribute filters, typed names), renamed, deleted (filter refunded), moved down; same after reload |
| 2 | Stock keeper screen | Pass | Categories fold; search by name, `@create`, `#logs`; wheel scroll clipped to the list; lock toggles; addresses offered from a held and a placed clipboard; seated villager drawn facing the cursor; Send made a 2 iron + 1 diamond package to `Storage` |
| 3 | Signs as addresses | Pass | Sneak-placed sign "Bakery" stays on a packager, its package is addressed `Bakery`; wall signs stay on a frogport and a postbox |
| 4 | Potions through drain, pipe, tank, spout, bottle, open end | Pass | Custom colour and effects (jump boost II) kept to the filled bottle and given by the open end; tank and glass pipe drawn magenta |
| 5 | Harvester, roller, portable storage and fluid interfaces | Pass | Moving parts drawn placed, on a bearing and on a train |
| 6 | Actor heads on a bearing and a train | Pass | Drill, saw, harvester, roller turn between frames; deployer drawn |
| 7 | Contents on moving structures | Pass | Depot item, tank and basin water drawn on a turning bearing; depot item on a train |
| 8 | State across a piston move and a bearing stop | Pass | Depot items, tank and basin fluid, motor speed equal in `/data get` before and after; link frequencies and funnel filter still drawn |
| 9 | Elevator contacts and floors | Pass | Doors open for All, one side and None as set; contraption controls call a floor |
| 10 | Sloped belts | Pass | Sloped outline; items ride up and down |
| 11 | Station summary and factory gauge status sources | Pass | Board shows `now / Unnamed Train / Zeta`; gauge board shows `9 ▪ Diamond` (boards need 30 rpm, as Create's) |
| 12 | Clipboard copy and paste | Pass, after fixes | Funnel filter and count, chassis range, brass tunnel side filter (per side), steam engine direction, table cloth price copied and pasted, read back by copying again. Fixed: filter items on east and west value boxes were drawn inside the block (`0f4bd3b`); a chassis' range board did not open with the wrench, which turned it instead (`de419ed`) |
| 13 | Glass pipe flow and drips | Pass | Flow grows from the pump, drips at the open end, clears when the pump stops (as Create) |
| 14 | Open pipe ends | Pass | Milk clears poison, tea gives haste, potions their effect; water puts out fire and burning mobs and places a source, lava sets fire and places a source |

### Families

| Family | Variants | Result | Evidence |
|---|---|---|---|
| Shafts | andesite, brass, girder encased, 3 axes | Pass | Motor drives all, 16 rpm |
| Cogwheels | small, large, encased, shafted | Pass | Speeds pass; cog on shaft extends it |
| Gearboxes | gearbox, vertical gearbox | Pass | Reverses straight through |
| Clutch, gearshift, sequenced gearshift | powered and not | Pass | Clutch stops, gearshift reverses, sequence turns 90° |
| Speed controller | | Pass | Large cog above takes its speed |
| Motors, crank, valve handles | creative motor, crank, copper + 16 dyed valves | Pass | Crank 32 rpm 256 su; valve turns 90°; dyeing |
| Water wheels | small, large, 12 woods | Pass | 8 rpm 256 su / 4 rpm 512 su |
| Windmills | bearing, frame, 16 sails | Pass | 16 sails 2 rpm 1024 su, drawn turning |
| Steam engine and boiler | 5 engine faces, boiler sizes | Pass | Level 1 boiler, 64 rpm 16384 su |
| Gauges | speed, stress, 24 states | Pass | Dials on Create's faces, needle reads |
| Belts | lengths, slopes, casings | Pass | Items carried to a funnel |
| Funnels | andesite, brass, 24 states, belt funnels | Pass | Insert and extract; filter drawn |
| Tunnels | andesite, brass | Pass | Items pass; side filters |
| Chutes | 20 states, smart chute | Pass, after fix | A chute fed by a hopper never let the item fall (`8f9a74c`) |
| Depots, ejectors | | Pass | Ejector throws onto a depot |
| Mechanical arms | floor, ceiling | Pass | 16 apples moved depot to depot |
| Item vaults | 2×2, 3×3 | Pass | Hopper fills |
| Observers, switches | content observer, stockpile switch | Pass | Powered by chest contents |
| Press | | Pass | Iron sheets |
| Mixer, basin | | Pass | Andesite alloy at 32 rpm, idle at 16; basin output and filter |
| Blaze burner | states, coal, cake | Pass | Kindled, fading, smouldering, seething |
| Saw | 24 states | Pass | Stripped log |
| Drill | | Pass | Breaks stone |
| Millstone | | Pass | Flour |
| Crushing wheels | | Pass | Gravel |
| Fans | washing, smoking, blasting, haunting | Pass | All four recipes |
| Deployer | 12 states | Pass | Filter, crafting cogwheels |
| Crafter | 16 states | Pass | Planks |
| Pumps, pipes, valves | shapes, encased, glass, smart pipe, valve, 6 pump facings | Pass | Flows and filters |
| Tanks | 2×2×2, 3×3×3, 1×4, creative | Pass | Fill drawn, kept on reload |
| Spout, drain | | Pass | Buckets and bottles filled; drain takes a bucket |
| Hose pulley | | Pass | Drains a pool into a tank |
| Bearings | mechanical, windmill, clockwork | Pass | Assemble, turn, clock hands |
| Pistons | normal, sticky, poles | Pass | Push, retract, pull |
| Pulleys, elevators | rope, elevator | Pass | Load lowered; floors |
| Gantry | carriage, shafts | Pass | Runs both ways |
| Carts | assembler, cart contraption, couplings | Pass | Drill cart assembled, coupling drawn |
| Trains, stations, signals | track, curves, stations, bogeys, signals, observers | Pass, after fix | Carriages lost their glue and controls' settings when taken apart (`09011b2`); trains run between stations, signals and observers react |
| Moving actors, glue, sticker, seats, controls | harvester, plough, roller, drill, saw, interfaces | Pass, after fix | Glue clicked on a deployer or depot used the block (`26d0af2`); actors break, cut, harvest; interfaces move items |
| Packages, frogports, chains | packager, repackager, frogport, chain conveyor, cardboard | Pass | Placed and drawn; packager makes addressed packages |
| Stock | link, ticker, gauge, requester, table cloth shops | Pass | Rows 1, 2, 11, 12 |
| Redstone | links, latches, pulse diodes, analog lever, contacts, nixies (16), display link and board | Pass | Link pair transmits; latch holds; extender extends; facing contacts power; nixie shows `15` |
| Postboxes | 16 colours, 4 facings | Pass | Drawn; keep signs |
| Decor, stone, copycats | casings, 14 stones × cuts, ladders, bars, scaffolds, windows, doors, cloths, toolboxes, seats | Pass | Drawn; doors open; seats, toolboxes dye; copycats take blocks |
| Equipment | goggles, wrench, grip, potato cannon, wands, backtanks, diving gear, cardboard armour, foods | Pass | Drawn; goggles overlay on a motor |
| Schematics | table, cannon, schematic items | Pass | Screens open |

### Kinetics against Create's Java

The [create-fuzz](https://github.com/OlaHulleberg/create-fuzz) differential fuzzer runs random kinetic networks in
Create 6.0.11 (NeoForge 1.21.1) and in the engine and compares where they settle. Since the kinetic propagation was
ported from Create (#9: sources, networks and Create's order) and its state is saved and loaded as Create does (#10),
10299 of seeds 0-10299 match, comparator signals included; the one left is #7, which waits on the mod API. With network membership and sizes observed too (#12), blocks changed in place (#13, #15) and the day time set per case, 19996 of seeds 0-19999 match with none unstable; the four left (5215, 6655, 15359, 19199) are #7. Each fix, tried in game on the build
above:

| Change | Result | Evidence |
|---|---|---|
| A cog loop whose ratios disagree, nothing turning | Pass | Two small and two large cogs placed with `/setblock`: none breaks (one did before) |
| A motor placed facing a turning motor | Pass | Placed by the player and with `/setblock`: both stay; the second shows `Gen: -16`, turning at 16 with the first; the shaft turns between frames; same after reload |
| Three large cogs at right angles, then a motor | Pass | The x and z cogs break and drop, the y cog stays still, as in Create |
| A motor's speed set to overspeed the cogs it drives (#2) | Pass | Small and large cog, motor on the large one; 183 rpm pasted onto it from a clipboard: the large cog breaks, the motor runs at 183 |
| Kinetic propagation ported (#9) | Pass | Press line at 16; member motors placed by the player and with `/setblock` (`Gen: -16`, turning at 16); three large cogs: x and z break, y stays still; motor pasted to 183 breaks the conveying cog |
| A gauge's comparator after its network stops (#11) | Pass | Hand crank on a speedometer read by a comparator: 7 while turning at 32, 0 once the crank stops |
| A split shaft is one network member (#12) | Not gameplay-visible | Network size has no in-game readout and clutches and gearshifts add no stress; checked by the fuzzer (5299 of 5300, the other #7) and `split_shaft_is_one_network_entity` |
| A kinetic block's state changed in place (#13) | Pass | Motor at 16 facing north, `/setblock` to facing south beside a shaft: it keeps -16 and the shaft stays still, as in Create; after reload it turns at 16 and drives the shaft (`CreativeMotorBlockEntity.initialize`). A player's wrench, clicked outside the value box, turns a motor from east to down: 16 becomes -16 on the next tick (`switchToBlockState`'s `reActivateSource`) and stays -16 after reload |
| A speed change never re-enrols a detached member (247556e) | No gameplay path known | `KineticBlockEntity.setSpeed`/`onSpeedChanged` leave the network alone; `detached_member_speed_change_does_not_enroll_it_again` fails without it; fuzzer seeds 0-10299 unchanged |
| A motor changed in place to another axis (#15) | Pass | Motor up at -32, `/setblock` to west, then 32 pasted from a clipboard: turns at -32 with network capacity 0 and added capacity 16384, as in Create; after reload capacity 524288 (`initFromTE`, `addSilently`, `updateCapacity`). Right after the `/setblock` the saved data has no `Network`, as Create's `write` (it was merged, keeping the old one); reload turns at -16 in a fresh network |
| A block entity's owned data written whole (#16) | Pass | A table cloth given `Shop` and `Price` with `/data merge`, then an apple put on it by the player: its saved `TableCloth` is rewritten as Create writes it, without them (a merge kept them). Motor `/setblock` up → west: no `Network` left in `KineticState`; reload turns at -16 |
| A busy valve handle's value box (#22) | Pass | Copper valve on stone, turned at 32 by an empty-hand use, then red dye clicked on its centre while it turns: it becomes red (`showValue` hides the box while in use). The first build crashed on launch (a nested client-table borrow in `render_gui_post`), fixed before this check |
| A sequence context cleared when its run ends (#19) | Pass | Copper valve handle used once (45°): saved `Sequence` while turning at 32; once the use expires it stops and has no `Sequence` (it kept one before, `HandCrankBlockEntity.tick`) |
| A valve handle recoloured while turning (#21) | Pass | Copper valve on a shaft, used and dyed red within its use (ticks frozen across the two clicks): handle and shaft keep turning at 32 after the use ends, as Create in the same input (create-fuzz); dyed after the use ends, both stop. Still 32 after reload (not compared with Create) |
| A sequencer's next instruction attaches on the following tick (#20) | Pass | Motor, sequenced gearshift (default program) and shaft, powered by a redstone block: the shaft turns at 16, stops when the 90° turn ends, and the gearshift keeps state 1 while powered (`run(-1)`); the one-tick attach timing itself is checked by create-fuzz |
| An adjustable chain gearshift's flicker under signal changes (#18) | Pass | Motor and adjustable chain gearshift, a redstone block above toggled every 4 ticks (`/tick freeze`, `step 4`): it breaks at the 21st toggle, as Create does for the same input (create-fuzz) |
| Default states from Create (#17) | Pass | `/setblock` with no properties: encased chain drive `axis=y,axis_along_first=true,part=none`, clutch `axis=y,powered=false`, as Create's runtime defaults; a player-placed chain drive keeps `axis_along_first=true` after reload; fresh worlds load without errors |
| A pulley put into a driven belt (#24) | Pass | Motor, two x shafts joined by the belt connector (four segments, 16 rpm), a shaft used on the second segment: it becomes a pulley and the segments keep turning at 16 with network capacity 262144 (it was 0) |
| A belt's break cascade (#23) | Pass | Four-segment belt made with the connector, a pulley in the second segment, the start broken with ticks frozen: one tick later air, shaft, air, shaft, as Create for the same input (create-fuzz); the same after reload |
| Empty running belts tick (#25) | Not separately visible | A 1/8-block start offset and a one-tick handoff; checked by create-fuzz (both cases match, 129 seeds changed to match, none newly differ) |
| A depot used with the same item (#26) | Pass | 16 iron put on a depot, then one more iron used on it: the depot holds 17 and the hand is empty, as Create (`SharedDepotBlockMethods.onUse` gives back before placing) |
| A belt stack behind one that enters a depot (#27) | Not separately visible | One tick of follow-on movement; checked by create-fuzz (the three item cases match, 129 seeds changed to match, none newly differ) |
| A gearbox's and a split shaft's own speed (#28) | Pass | Motor east into a gearbox: the gearbox's saved speed is the incoming 16, the side shaft 16, the far shaft -16, the same signs as Create for that input (create-fuzz), and the same after reload |
| A crank's or valve's use saved and restored (#29) | Partly in game | A crank turned with ticks frozen saves `HandleState` with `InUse: 8` while turning at 32 (written only when it changes), and the world reloads without errors; the 8 ticks run out while the world loads, so the restore itself is checked by `crank_use_survives_cold_load`, `valve_use_survives_cold_load` and create-fuzz's reload cases |
| Saved network figures from the entity's cache (#33) | Pass | Motor and two shafts, one shaft removed, saved and reloaded: saved `Size` 1 on the motor, 3 then 2 on the shafts, the same after reload, exactly what Create saves for that input (create-fuzz: each entity's `networkSize` changes only when its network syncs) |
| Reloads keep depots' held items and network members (#30, #34) | Pass | A motor driving two shafts and a depot holding two diamonds (put on by the player), saved and reloaded: speeds 16, saved sizes 1 and 3, the depot still holding both diamonds, no errors; the sliding position and the transient member counts are checked by create-fuzz's reload cases |
| A loaded belt forms its controller before its first tick (#31) | Pass | A six-segment belt made with the connector, iron dropped on it, saved and reloaded: `BeltState` (controller, index, length 6) and speed 16 kept, no errors; item positions on the first tick after load are checked by create-fuzz's reload cases |
| Entity state published after every subsystem's tick (#32, #35) | Pass | A sequenced gearshift running its default turn, ticks frozen and stepped by 3: its saved `Timer` 3, 6, 9 and `InstructionProgress` 14.4, 28.8, 43.2 follow the live run each step (it was saved every 10 ticks); a depot filled by a belt is published after the belt phase, checked by create-fuzz's reload cases |
| A belt's saved inventory follows its items (#36) | Not separately visible | Belt items live in the column attachment, which `/data` can't show; checked by create-fuzz (the case matches: saved positions follow the moving items, published only when changed) |
| Loaded entities: sequencer timing and first-tick network enrolment (#37, #38) | Partly in game | A sequencer saved at tick 8 of its 21-tick turn reloads and ends its turn normally (the rest runs out while the world loads), with no errors; the continued timing and the enrolment order are checked by create-fuzz's reload cases, all matching |
| Fluid tanks split and reform as Create (#39, #40) | Pass | A 2x2x3 tank made with `/fill`, a lower corner broken: the upper 2x2x2 reforms as one tank (bottom at y+1, top at y+2) and the remaining lower block stands alone, as Create; window flags stay per member (create-fuzz) |
| Chain drives after a break, chain gearshift signal on load (#44, #48) | Pass | Three chain drives (start, middle, end), the middle one broken with ticks frozen: both ends are `part=none` after one tick, as `ChainDriveBlock.updateShape`; the saved signal on load is checked by create-fuzz's reload cases |
| Network caches of loaded entities (#43, #45) | Not separately visible | Cached network figures after a load (a ghost-only network, a handle expiring on its first tick); checked by create-fuzz (both cases match, 218 seeds changed to match); a world with a crank reloads without errors |
| Depots and belts tick in entity order; first empty-belt direction (#46, #47) | Pass | A 3-long belt at 16 rpm feeding a depot registered after it, ticks frozen and stepped: the arriving ingot is at `Pos` 0.125 on its first tick (it slid in the same tick, as `DepotBehaviour.tick` after the belt) and settles at 0.498; saved mid-slide at 0.219, it reloads and finishes its slide with no errors; the depot-first order and the empty belt's direction are checked by create-fuzz (both cases match, 49 seeds changed to match) |
| Fluid valve pointer: placed state and saves (#41) | Pass | A valve set with `enabled=true` and no rotation starts at `Pointer` 0 and is `enabled=false` after its first tick, as `FluidValveBlockEntity`'s constructor and `tick`; turned by a 16 rpm motor to 0.55, the motor removed (pointer stops at 0.6), saved and reloaded: `Pointer` still 0.6, the valve still shut, no errors; create-fuzz checks the reload case (matches, 19 seeds changed to match) |
| Interactions published when accepted (#42, #53) | Pass | Ticks frozen, a creative motor driving a shaft set from 16 to 96 rpm with a clipboard: the motor's and the shaft's saved `Speed` read 96 at once; saved and quit without a tick, the world reloads with both at 96 (before, the save kept 16); create-fuzz: 147 seeds of 0-1999 changed to match, none newly differ |
| A handle reloaded mid-use runs out as Create (#50, #54) | Pass | A copper valve handle on a shaft, used with ticks frozen: after 3 ticks `InUse` 4, network capacity 256; saved and reloaded, the use runs out and the shaft stops with no capacity left (before, 256 stayed); #54 was a create-fuzz driver bug (fixed there); create-fuzz: seeds 298, 314, 490, 506 changed to match, none newly differ |
| Tank reconnection and valve opening on the entity's tick (#51, #52) | Pass | A 2x2x2 tank from `/fill`, a lower corner broken with ticks frozen: one tick later the upper layer is one 2x2x1 tank and the lower blocks stand alone, as `removeController` → `updateConnectivity` on the next tick; a valve on a 16 rpm motor opens (`enabled=true`, `Pointer` 1.0) within a second, no errors; create-fuzz: seeds 182, 744, 1128, 1320, 1586, 1704 changed to match, none newly differ |
| Sequence, depot motion and generator updates across reloads (#56, #57, #58) | Pass | Ticks frozen: an item put on a depot is saved at once with `HeldItem` `Pos` 0.25 and, after a reload, finishes sliding to 0.498; a valve handle used is saved at once with `Sequence {Value 45, Run 0}` and `InUse` 7, and reloads without errors; #57 (facing motors reapplied before a reload) checked by create-fuzz; seeds 1322, 1338 changed to match, none newly differ |
| Directed speed modifiers along a chain (#55) | Pass | A creative motor (16 rpm) driving gearshift, drive, gearshift, drive, gearshift (axis z, chained along x): unpowered all 16; the first gearshift powered, the rest 32; both ends powered, 16 32 32 32 16; only the last powered, 16 16 16 16 8; no block breaks, no errors; which block a conflict destroys is checked by create-fuzz (the case matches; seeds 684, 700, 812, 828 changed to match) |
| Pending tank connectivity and network admission across reloads (#59, #60) | Pass | Ticks frozen, a player places a fluid tank on another: both stand alone (`top=true,bottom=true`) with connectivity pending; saved and reloaded, they form one 1x2 tank (`bottom=true,top=false` / `bottom=false,top=true`), as `Uninitialized` → `updateConnectivity`; network admission after an early reload checked by create-fuzz (both witnesses match, seeds 1256 and 1328 changed to match, none newly differ) |
| A valve handle recoloured mid-use keeps its source tree (#69) | Pass | A copper valve handle on a shaft, used (`InUse` 7, shaft 32 rpm), then dyed red with ticks frozen: it becomes `red_valve_handle`, keeps `InUse` 7 at 32 rpm, and the shaft's `Source` is still the handle; saved and reloaded, it stays red and comes to rest once the use runs out, no errors; the same-tick recolour and reload checked by create-fuzz (1386, 1402 changed to match, none newly differ) |
| Pipes connect to vanilla fluid targets (#67) | Pass | A fluid pipe placed by a player on the floor east of an empty cauldron becomes a straight east-west pipe (`east=true,west=true`, others false), as Create's connectivity to vanilla fluid targets; create-fuzz's witness matches; no seed changed status |
| Fluid machines in block-entity order; depot hold boundary (#65, #66) | Pass | A water bucket used on an item drain: the drain's tank reads 1000 of 1500 mB of water, no errors; the shared registration-order walk for drains, spouts, pumps and belts, the depot's held-item lock and drain turn timing are checked by create-fuzz (drain seeds 147 and others from 67 to 36 differing fields, none newly wrong; no seed changed status; pressure fields await #68) |
| Pipe connections with stored pressure (#61-#64, #68) | Pass | A creative motor drives a mechanical pump (facing east) through a cogwheel; a water source west of the pump through a pipe, a fluid tank east through a pipe: the pump's east connection saves `Pressure [0, 16]` with a flow in progress, and the tank fills 192 → 992 → 1000 mB of water within seconds while the source drains; saved and reloaded, the tank keeps 1000 mB; create-fuzz: 109 seeds of 0-1999 changed to match, none newly differ. Separate: with water placed in front of the pump's outlet the integrated server disconnects (also on 5dddd33, before this change), filed on its own |
| Drawn contents published only when they change (#70) | Pass | The test world that disconnected about 55 s after every load (a settled depot republishing its redraw marker every tick, 940 identical updates before the keep-alive timed out) stays connected past 120 s; the depot's `Shown` snapshot is unchanged between reads; a depot and a belt still redraw when diamonds are swapped for emeralds (agent's instance, 291 s connected) |
| Valve speed callbacks published before a save (#72) | Pass | Ticks frozen, a fluid valve set `enabled=true` beside a turning creative motor: after one tick its `Pointer` chases target 1.0 at 0.05; saved and reloaded it keeps target 1.0, reaches 1.0 and stays `enabled=true`, as Create's `onSpeedChanged` → `pointer.chase`; create-fuzz: both cases and seeds 7989, 7990, 7993, 10292-10297 match, no seed changed status in 0-1999 |
| Pipe pressure kept across a reload (#73) | Pass | Two mechanical pumps in series, each turned through a cogwheel by a creative motor, a pipe between: the pipe's connections hold `Pressure [0, 32]` / `[32, 0]`; saved and reloaded they read the same, and still the same 10 s later (before, a reload doubled them); create-fuzz: seeds 13810 and 17330 match, no seed changed status in 0-1999 |
| Sunrise nightly pin (2d81263) | Pass | Official nightly client, fresh world: a creative motor turns a shaft at 16 rpm, a belt carries a diamond onto a depot, a pump fills a fluid tank to 8000 mB, a valve opens (pointer 1); saved and reloaded, all four keep their state and the tank keeps filling |
| Clutch and gearshift placed beside power (#7) | Pass | Nightly game: a clutch and a gearshift placed by the player and by /setblock beside a powered block place `powered=true`; one set `powered=true` with no signal unpowers on its next tick; create-fuzz main 62f53ab: `fuzz 0 2000` 2000 matched, the 13 `powered` seeds of the 42c51d2 sweep fixed |
| Nightly APIs, blocks and world | Pass | Lava beside honey/chocolate (source → obsidian, flowing → limestone/scoria), by /setblock and bucket, kept on reload; drill and deployer breaks and a water wheel's material change play the destroy-block effect; symmetry wand mirrors fit fences and walls; wrench turns fit neighbours and survive reload; hand crank, valve handle, placard and factory gauge break when their support is removed, survive against fence collision in all four facings; factory gauge outlines kept on reload; requester placed by a deployer keeps its address; toolbox, backtank and roller placed facing south and east keep their states over two reloads; schematic export and print keep item frames and paintings |
| Nightly APIs, items, entities and client | Pass | Schematicannon shift-click sends a schematic, books and 64 gunpowder to slots 0, 2 and 4, kept on reload; deployer with a diamond sword hits a cow for 7 (full charge); cardboard sword hit stops a sprint; `/tick freeze` + `/tick step 5` keep a turning shaft at rest, `/tick unfreeze` resumes; fan pushes faster at 256 rpm; depot and belt draw stone 3D and diamonds flat; extendo grip pose with a block in the main hand; turning bearing, piston and clockwork bearing structures lit by the world around them; a bearing stopped mid-turn keeps its angle (43.2°) over a reload (fixed in 296b723). Not observable here (no audio device): bonk and fan pitch |
| A windmill's angle saved (#80) | Pass | Nightly game: a windmill (16 sails) turning at 2 rpm keeps its angle over two save/reloads (208° then 351°, on its 0.6°-a-tick line); taken apart after a reload its sails go back unturned, also with a glued drill; a mechanical bearing reversed 8 → −16 rpm rounds its angle to a whole degree (276.00012 → 276), and stopped in never-place mode it keeps −46° over a reload; create-fuzz `fuzz 0 2000` 2000 matched |
| Chain drives join and re-link in the block change (#94, #97, #98) | Pass | Nightly game: breaking a drive's chain partner re-links it with the drive above at once (start/end along y); a lone drive wrenched into line with a gearshift becomes x/end and the gearshift start in the same tick, both turning; gearshift, drive, gearshift placed by a player join as start, middle, end at once; breaking the middle drive leaves both ends `part=none`; all kept over a save and reload; create-fuzz (kgap-network) `fuzz 0 2000`: 54 seeds fixed, none newly differing, the six issue cases match |
| Generators keep their state over a reload (#89, #90, #91) | Pass | Nightly game (ed3ac19): a flowing water wheel (8 rpm, `FlowScore 1`, network 256/1), a large water wheel (4 rpm, network 512/1, 8 frame cells) and a steam engine on a 2×2 boiler with 4 kindled burners (64 rpm, network 16384/1, `EnginePower 1.0`) come back the same after save/quit/reload (the boiler's supply is re-sampled, as Create's); breaking a large wheel's frame corner with ticks frozen and stepped: the wheel goes the next tick, the four cells beside it the tick after, the corners the tick after that, as `WaterWheelStructuralBlock.updateShape`/`tick` decide; create-fuzz (kgap-generators) `fuzz 0 2000`: 2000 matched, 86 seeds fixed, none newly differing |
| A sneak-use with an item is the block's own use on cranks, valve handles and levers (#96) | Pass | Nightly game (ed3ac19): a hand crank, a valve handle and an analog lever each used with an empty hand, a shaft and an extendo grip (sneaking and not) and the wrench: sneaking with an item turns the crank and the valve backwards and lowers the lever, nothing is placed from the held stack, the wrench keeps its own use; states kept over a save and reload, clicks still work after it; create-fuzz `fuzz 0 2000`: 31 seeds fixed (the held-crank family), none newly differing |
| Bearings assemble on a speed change, not on a reload (#106) | Pass | Nightly game (ed3ac19): a mechanical bearing assembles when powered, comes apart by an empty-hand right-click (aimed off the side faces' value box, where Create puts it), and stays apart over a reload while still powered (before, every reload re-assembled what stood in front of it); assembled again by right-click; reloaded while turning it keeps `Rpm: 16` and comes apart; a windmill keeps `Rpm: 3` over a reload; a player-placed bearing and a glue box assemble; create-fuzz `fuzz 0 2000` 2000 matched |
| A wrench clicks an analog lever without sneaking (#111) | Pass | Nightly game (ed3ac19), the lever's level read from a redstone wire beside it: a standing wrench clicks it up (7 → 8 → 9), a sneaking wrench leaves it (7), an empty hand or a stick clicks up standing and down sneaking; the lever is never picked up; its level survives a save and reload (7, then 9); create-fuzz `fuzz 0 2000` 2000 matched |
| Sequenced gearshifts run from their scheduled block ticks (#99, #100) | Pass | Nightly game (ed3ac19): two sequenced gearshifts in a line powered by one `/fill`: the first starts (`state=1`, instruction 0), the second stays idle (`state=0`, `InstructionIndex -1`, `Duration -1`, `PrevPowered 1b`), as Create's `willTickThisTick` decides; a programmed gearshift run to its end, unpowered, saved, reloaded and repowered restarts its program (`state=1`, instruction 0, its far half turning); player placement and `/setblock` give the same state and data; create-fuzz (network inputs): the 31 `sequencer_pair` seeds fixed, none newly differing |
| Network size around reloads and between opposed motors (#95, #101) | Pass | Nightly game (520fc48): a shaft placed between two motors turning against each other breaks with its drop, and both networks keep ±16 rpm with their sources and size 2; a gearshift powered after a reload flips its output to -16 with the input at 16, and a relaunch with the power in place keeps `powered=true` and +16/+16/-16; create-fuzz `fuzz 0 2000` 2000 matched |
| Belt slicer follows BeltSlicer (#130, #131 except f2b, #132, #133) | Pass | Nightly game (520fc48): a belt shortened from its START and END with the wrench, lengthened and split in creative (the cut-off half stays still while the driven half carries its items) and joined again with the connector after a save and reload; a survival split is refused without a connector; create-fuzz `fuzz 0 2000` with main's driver unchanged from main |
| Belt tunnels, funnels, chutes, inventory and item handler (#134-#140) | Pass | Nightly game (520fc48): wrench-closed andesite and brass tunnel windows stay closed through 60 ticks and a neighbour change; breaking a segment takes its belt and four tunnels in one tick; a belt funnel fills a depot and the rest queue; a hopper inserts into a belt and stops when the segment is blocked; stacked chutes fill an empty depot and wait in the middle over a full one; items picked off a stopped belt are gone within 3 ticks; all kept across a save and reload; create-fuzz `fuzz 0 2000` fixes 21 seeds, none newly differing |
| A large water wheel's frame cell hands the sneak wrench to its wheel (#165) | Pass | Nightly game (520fc48): a sneak wrench on an edge frame cell takes the wheel into the inventory and the frame goes a moment later; a small wheel turned beside flowing water reads flow 0, speed 0 and keeps it across a relaunch; create-fuzz `fuzz 0 2000` 2000 matched |
| Sequenced gearshifts tick in block-entity order (#173) | Fuzz only | create-fuzz case and seeds 2356, 4788, 7220, 9652, 12084 match; not reproduced in game on `9425651` (see that smoke test, item 7) |
| Brass tunnels, funnels and the logistics tick order (#143-#149, #154) | Pass | Nightly game (`9425651`): see that smoke test, item 8; create-fuzz `fuzz 0 2000` fixes 26 seeds |
| Kinetic state saved and loaded (#10) | Pass | Two member-motor pairs keep their roles over two save/reload cycles; a world saved before #10 loads with both pairs intact and keeps them after saving again |

Known differences left: the category editor's inventory shows the inventory as it was at the last change made in
it, so items picked up while it is open appear after the next change (Create's menu syncs them at once). The game
dropped to the title screen ("End of stream") five times across three test sessions with no panic or crash report;
none reproduced.

## Potato projectile on `9425651`

`tools/check-potato-projectile.py` ran with a native package staged from the signed SDK, the pinned jar, and a new world. The first shot took a pig from 10 to 5 health, consumed one inventory potato and wore the cannon by one point. A shallow shot planted one potato crop. A cake from the off hand took a pig to 2 health, consumed the cake and left the potatoes alone.

The disposable fixture rejects a join, then in another world switches to an iron pickaxe from the join callback; neither case spends ammo, wears either item, or hits the target. The first position and velocity after reopen equal one flight step from the final pre-save sample, with identical projectile and owner UUIDs. The client renderer reads the synced ammunition. A stuck cake follows its target exactly one block sideways. All three sweet-berry projectiles hit the same wall on their first step and spend one berry and one durability point in total.

To reproduce on Linux, set `GAME`, `CTL`, `RESOURCES` and `JARS` to absolute paths for the signed Nightly client and matching control tool, a private prepared resource tree, and the verified jar directory. From the repository root:

```sh
(cd create && xvfb-run -a cargo sunrise-mod run --game "$GAME" --resources "$RESOURCES" --jars "$JARS" -- --frames 1)
xvfb-run -a python3 tools/check-potato-projectile.py --ctl "$CTL" \
  --game create/target/sunrise-mod/run/app/sunrise-client --resources "$RESOURCES" \
  --jars "$JARS" --scratch target/potato-smoke
```

In a **disposable checkout**, apply `git apply tools/fixtures/potato-projectile.patch`, repeat the staging command with that checkout, and append `--fixture` to the test. Do not ship the instrumented build. Client prediction and Create's ammo-specific render poses are not implemented.

## In-game smoke test on `9425651` (main `684e89b`)

Built with `sunrise-launcher native-build` against `nightly-2026-11-20-9425651`, resources prepared by that build's
`sunrise-prepare` (output format 5; the format-4 resources crash its client with `missing field 'build'`), a fresh
creative world, driven by `sunrise-ctl`.

| # | Item | Result | Evidence |
|---|---|---|---|
| 1 | Creative motor turning shafts | Pass | 16 rpm on both shafts; motor `Speed: 16.0f`, capacity 262144 |
| 2 | Belt carrying a diamond onto a depot | Pass | 5-long belt made with the connector by a player; the depot holds the diamond after about 4 s |
| 3 | Pump filling a tank | Pass | 8 mB per tick from a pool, 2288 → 3448 mB, then full; a single source gives 1000 mB and becomes air; a bucket puts 1000 mB in; with air ahead the pump places a source |
| 4 | Fluid valve opened | Pass | Pointer `Value: 1.0f`, `enabled=true` |
| 5 | Bearing and windmill | Pass | 4 glued planks turn at 16 rpm; a 24-sail windmill at 3 rpm, 1536 SU |
| 6 | Player placement and breaking | Pass | Shaft (`axis=x`), gearbox (`axis=y`) and cogwheel (`axis=z`) placed against turning blocks turn at 16 rpm; broken in 1-2 ticks with the neighbour still turning; placed again and turning |
| 7 | Sequenced gearshifts saved at a program's last tick (#173) | Not reproduced | After a relaunch every block read 0 rpm and 0 SU, the second gearshift with no network. Java's 256 SU needs the run to end on the reloaded tick; here about 21 ticks passed before the first read and the crank may have stopped before the save. The fuzz case for #173 matches |
| 8 | Brass tunnels and funnels (#143-#149, #154) | Pass | Split, round robin and synchronize each delivered in Create's order to three depots; a tunnel replaced in a synchronizing group took its mode; a belt funnel pushed a chest onto a belt and waited when it backed up; a funnel over a depot pushed one ingot and kept the rest; a pulling funnel took a depot's item into a chest |
| 9 | Save, quit, relaunch | Pass | Speeds, depot items, valve, bearing and windmill (still turning), tunnel modes and funnel shapes kept, and all went on working; the tank went on filling |

Game log: no mod errors or crash reports once the resources matched the build. Unconfirmed: a sneak-placed funnel came
out extracting (Create's `FunnelBlock.getStateForPlacement` makes it non-extracting, as the port's placement does);
whether `ctl place` sent the sneak was not checked.

## In-game smoke test on `520fc48` (main `6373106`)

Built with `sunrise-launcher native-build` (launcher 1.1.1; 1.0.1 rejects the scaffolding data's
`context_collision` field) against `nightly-2026-11-19-520fc48`, a fresh creative world, driven by `sunrise-ctl`.

| # | Item | Result | Evidence |
|---|---|---|---|
| 1 | Creative motor turning a shaft | Pass | `create_debug speed`: 16 rpm, stress 0 / 262144 su; motor and shaft `Speed: 16.0f` |
| 2 | Belt carrying a diamond onto a depot | Pass | 5-long belt at -16 rpm; depot `Items: [{count: 1, id: "minecraft:diamond"}]` |
| 3 | Mechanical pump filling a tank | Pass | One source: 1000 mB; refilling pool: 48 → 544 → 1048 → 1544 → 2040 → 2544 mB at 3 s intervals, then 8000 (full) |
| 4 | Fluid valve opened | Pass | Closed: tank 0 for 8 s; driven at 16 rpm: `Pointer` value 1.0, `enabled=true`, tank 824 → 2302 mB |
| 5 | Bearing and windmill | Pass | Super-glued planks assembled at 16 rpm; 8-sail windmill at 1 rpm, 512 su; both seen turning |
| 6 | Player placement and breaking | Pass | Shaft, cogwheel, gearbox and side shaft placed against blocks turn at 16 rpm; breaking the cogwheel stops the gearbox, re-placing restarts it |
| 7 | Save, quit, relaunch | Pass | All 14 speeds read back the same; depot keeps its diamond and takes the next once emptied; tanks keep 1000/8000 mB, a two-high tank goes on filling; bearing (`Angle: 48.0f`) and windmill keep turning |

Game log: no panics, crash reports or Create errors.

## Issue #17: declared-property defaults only (2026-11-17)

Branch `fix/issue-17-declared`, based on `f4a7f18`, leaves the held `fix/issue-17`
branch at `681df08`. Only properties declared by a block take their default from
Create's runtime measurement. The measurement is byte-identical to the one documented
in `41e60e4`: create-fuzz's server writes `work/default-states.json` at startup.
Main's existing partial `TEMPLATE_PROPERTIES` table and lookup are unchanged, and every
emitted override for its known inherited properties was compared against main: no changes.
No vanilla-report extract or new template facts are included.

Two exact pending lists contain one reason per block/property pair, each awaiting the
owner's template-data decision: **975 unlisted properties across 263 blocks**, and
**9 differing inherited defaults across 9 blocks**. The first includes the previously
identified 61 modelling omissions and properties the partial table cannot resolve.
These lists do not emit defaults or change main's inherited-property handling.

Default keys must exist among a block's declarations or main's table. Shape references
are checked there too, with explicitly tracked unresolved Create runtime properties
remaining pending. Spout, turntable and frogport shapes are constant as their Java
`getShape` methods specify; they need no artificial state selector.

- Rust 1.99.1 formatting and Clippy (`--all-targets -- -D warnings`) pass for engine
  and generator; generator Clippy/tests use `--locked`.
- All 691 engine tests and 49 generator tests pass, including both asset licence checks.
- The chain placement regression fails with main's old declarations; the property-key
  regression rejects the held branch's invalid pillar defaults; the domain-order
  regression fails when declared defaults revert to first-value selection. All pass
  with the narrower fix.
- Issue #17's shrunk case matches Java, including all checkpoints. The current fuzzer
  was copied to scratch, with its hardcoded chain default replaced by the generated
  declaration's value. Shared create-fuzz and its server were not edited or restarted.
  The full seed sweep was not repeated for this narrower follow-up.
- `hz_rebuild` succeeded. With no live non-zombie game process present, `FRESH=1
  hz_launch t17b` reached `screen=playing`, joined the local world and advanced to tick 19.
  No new crash report appeared during launch or after `hz_quit`. The tested declarations
  match the final generated declarations exactly.
