# What the port needs from the mod API

The mod targets the arismoko/horizon fork's mod API at the build `create/horizon-mod.toml` pins
(`42c5f703e3`, the Mod SDK of 2026-11-14). Every subsystem of the earlier mixin-based port is on the API;
this page lists what the API could not express when it was ported, so those behaviours are left out
(each module's doc says the same where it applies). Re-check a line against a newer SDK before acting
on it: `cargo horizon-mod sdk diff <old> <new>` lists what changed, and the fork moves fast.

Each line: what Create does, what the legacy glue called, and the API addition that would close it.

## Gaps found porting

### World, saving and blocks

- **A moment before a chunk is saved** ((removed)). The glue writes each changed
  column's data into its chunk attachment at the end of the tick that changed it (`glue/persist.rs`);
  a pre-save hook would let it write only when the game saves.
- **A world-open phase.** `world_lifecycle` reports only failures and closing; Create's world opens at the
  first `level_load`.
- **Block-state facts:** `isRedstoneConductor` and `isSolidRender` (cart assemblers, stock links, schematic
  export approximate them by "full collision cube"); `requiresCorrectToolForDrops` / `isCorrectToolForDrops`
  (deployers dig every block at its harvestable speed); the weathering-copper class and age; a block's class
  (copycats guess stairs, bars and trapdoors from tags and ids).
- **A state's numeric id**, for `level_event(2001, ..)` (block-break particles on a wrenched copycat).
- **The neighbour-shape update of a state placed by code** (`updateShape` against its neighbours): mirrored
  placements of the wand of symmetry skip it.
- **Waterlogging in `state_for_placement`**: the event carries no fluid at the position, so rails and display
  boards are not waterlogged when placed in water.
- **`OnBlockPlacedByEvent` without a level**: decor's fix-ups after a dispenser places a block are skipped.
- **`lightning_strike` names no level**, so lightning cannot clear Create's copper from the glue. Emitting
  `neoforge:oxidizables` / `neoforge:waxables` data maps for Create's copper (`tools/create-data`) would let the
  fork's own weathering, waxing and lightning handle it.
- **Fluid interactions in content declarations**: honey and chocolate meeting lava (obsidian from a source,
  limestone or scoria from flowing lava, `registerFluidInteractions`) have no declaration.

### Containers, items and transactions

- **Rolling back mod-owned state with a transaction.** A handler cannot learn that its transaction aborted,
  so Create's tanks (whose contents live in the engine) offer no `FLUIDS` capability, and packagers and
  frogports offer no `ITEMS` handlers to hoppers, funnels and chutes. A close/abort callback on `Transaction`
  (or a journal for mod state) would close both.
- **A notice for any container's contents changing.** `contents_changed` reaches only the changed block's own
  parts, so a chute under a vanilla container and a frogport above an inventory re-check on a timer instead.
- **Per-slot insert rules on `ModContainer`**: "insert only while empty" (depot, ejector, crafter, chute) and
  the toolbox's one item kind per compartment.
- **Enchantment maximum levels on the server** (the `max_enchanted` filter attribute).
- **`ConfigFiles::remove`** (deleting the oldest schematic upload), and a mod reaching the game folder's
  `schematics/` (uploads live in the mod's own data folder).

### Entities and moving structures

- **Moving colliders**: players cannot stand on, or be pushed by, bearings' and pistons' structures, trains or
  minecart contraptions (`notes/moving-structures-note.md`).
- **Mod entity types that are not template copies**, rideable by players: Create's seat entity is stood in by
  a tagged display entity; carriage seats, riding a moving structure's seats and clicking its blocks are out.
- **World-space drawing of any block state at a pose**: trains, minecart contraptions and couplings are
  simulated, saved and driven, but not drawn. Moving structures anchored to a block entity are drawn by it,
  lit with the anchor's light (no per-block light in a block entity's draw).
- **`MinecartRef::set_carrying`** (a cart with a contraption counting as ridden) and **`EntityRef::move_by`**
  (moving a coupled cart off its rail with collisions).

### Client

- **Whether an item's model is 3D** (`BakedModel.isGui3d`): belts, depots, placards and link slots approximate
  it by "the item places a block".
- **Keeping only some of a block entity's drawing**: `keep()` keeps everything drawn for it, so a block with
  one animated part is drawn every frame (the glue decides through `Subsystem::animated`). A way to hide
  another part's model would let the clockwork bearing's hour hand replace its top.
- **Text alpha in `BlockEntityDrawContext::text`** (display boards' 0.85 / 0.975), and more than one colour per
  line in tooltips (goggles).
- **A mutable `ClientHost` in screen hooks**: the clipboard screen closes and sends its pages from the next
  client tick.
- **Reading and setting the local player's movement on the client** (diving boots sinking), **consuming a key
  in `input_key`** (driving a train), and **a client-side left-click-block hook** (the cardboard sword's bonk
  is played from the server).
- **A global hit hook naming the attacker** (an extendo grip in the off hand wearing on hits).
- **Translations on the server** (`hover_name`, `translate`): display links use Create's en_us text.

## Left out of the port, not for want of API

- Fluids: the rest of Create's `use_plumbing` (copper casing on pipes, the smart pipe's filter slot,
  growing a whistle) and clicking fluid tanks (`use_tank`).
- Carriage seats wait on rideable mod entities (above); the seat helpers they need come back with them.

## Not to send

- The earlier port's `feat/create-hooks` branch on OlaHulleberg/horizon added `native_*` hook points for the
  mixin bundle. The fork's API replaced that route; none of it is needed.
