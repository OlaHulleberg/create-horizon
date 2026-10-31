# Porting Create 6.0.11 to Horizon (Nightly 9d979ca, MC 26.3)

Source of truth for behaviour: this repo's Java tree (`src/`, branch `main`, NeoForge 1.21.1); the port lives in `horizon/` on branch `port/horizon`.
Target: a horizon-mixins native bundle in `horizon/`, plus new hook points upstreamed to Horizon on
`/home/techi/repos/horizon` branch `feat/create-hooks` (based on `9d979ca`, `NATIVE_HOOKS_API` 40).

## Licensing (read before copying anything)

- Create **code** is MIT: it may be read and translated to Rust (Horizon CONTRIBUTING "Only original code").
  Keep Create's MIT notice in `LICENSE`/`THIRD_PARTY_NOTICES`.
- Create **assets** (`src/main/resources/assets/`, textures, models, lang, sounds) are **All Rights Reserved**:
  never commit them here or in Horizon. Load them at runtime from the player's own Create jar
  (as `horizon-port-pack` carries data from jars), or ship placeholder art.
- Registrate (MPL-2.0) and any GPL/LGPL addon source are off limits, even to read. Flywheel/Ponder:
  check each LICENSE at the pinned commit before reading.

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

1. **Block entity renderer hook** (client): per-frame draw of baked part models with a transform
   (rotating shafts/cogs/belts/press heads). Without it nothing in Create visibly moves. Biggest blocker.
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

## Phases (each ends playable and committed)

0. Setup: rename template (`create`), repin to `9d979ca`, stage+check+test green, asset loader from the
   player's Create jar (no ARR assets in git).
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
