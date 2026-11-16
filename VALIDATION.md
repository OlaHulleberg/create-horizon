# Validation

Run on 2026-11-16 (Linux x86_64, Rust 1.99.1), the mod pinned to Sunrise `b77574eb1b7ef6b8a36456ec890a3d2ba3af37a6`.
Sunrise has published no SDK for this pin yet, so `cargo horizon-mod check` used `cargo-horizon-mod` built from
Sunrise at the pin with an SDK built from the same checkout, signed with a throwaway key and fetched through
`HORIZON_SDK_BASE` / `HORIZON_SDK_TRUSTED_KEY`.

## Checks

| Where | Command | Result |
|---|---|---|
| `create/` | `cargo +1.99.1 fmt --check` | Pass |
| `create/` | `cargo +1.99.1 clippy --all-targets -- -D warnings` | Pass |
| `create/` | `cargo +1.99.1 test` | Pass (81 glue tests) |
| `create/` | `cargo +1.99.1 test -p create-engine` | Pass (659 engine tests) |
| `create/` | `cargo horizon-mod check` | Pass ("Create uses 3570 items of the API … Made for b77574e.") |
| `create/src/engine/` | `cargo +1.99.1 fmt --check` | Pass |
| `create/src/engine/` | `cargo +1.99.1 clippy --all-targets -- -D warnings` | Pass |
| `tools/create-data/` | `cargo +1.99.1 fmt --check` | Pass |
| `tools/create-data/` | `cargo +1.99.1 clippy --locked --all-targets -- -D warnings` | Pass |
| `tools/create-data/` | `cargo +1.99.1 test --locked` | Pass (46 tests, `committed_assets` included, with Create's jar in the `prepare` cache) |

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

Known differences left: the category editor's inventory shows the inventory as it was at the last change made in
it, so items picked up while it is open appear after the next change (Create's menu syncs them at once). The game
dropped to the title screen ("End of stream") five times across three test sessions with no panic or crash report;
none reproduced.

## Not run

- **A player's build.** Without a published SDK for the pin, the launcher builds the mod only on a developer base
  built from Sunrise at the pin, and CI's `cargo horizon-mod check` fails at the SDK fetch.
