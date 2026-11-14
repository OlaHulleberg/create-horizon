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
- **A state's numeric id**, for `level_event(2001, ..)` (block-break particles on a wrenched copycat, a
  belt's casing taken off).
- **The neighbour-shape update of a state placed by code** (`updateShape` against its neighbours): mirrored
  placements of the wand of symmetry skip it.
- **Waterlogging in `state_for_placement`**: the event carries no fluid at the position, so rails and display
  boards are not waterlogged when placed in water.
- **`OnBlockPlacedByEvent` without a level**: decor's fix-ups after a dispenser places a block are skipped.
- **`lightning_strike` names no level**, so lightning cannot clear Create's copper from the glue. Emitting
  `neoforge:oxidizables` / `neoforge:waxables` data maps for Create's copper (`tools/create-data`) would let the
  fork's own weathering, waxing and lightning handle it.
- **Scaffolding's collision for a templated block**: Horizon gives `ScaffoldingBlock.getCollisionShape` (solid
  only from above, unless descending) and its dynamic shape only to `minecraft:scaffolding` by name; a copy
  without declared shapes has none (placing it fails, and an entity touching it stops the game). Create's metal
  scaffolding declares the stable shape, so it is stood on but not climbed through from inside; a declaration
  field (or class-based lookup) for scaffolding's context rule would close it.
- **Fluid interactions in content declarations**: honey and chocolate meeting lava (obsidian from a source,
  limestone or scoria from flowing lava, `registerFluidInteractions`) have no declaration.

### Containers, items and transactions

- **Rolling back mod-owned state with a transaction.** A handler cannot learn that its transaction aborted,
  so Create's tanks (whose contents live in the engine) offer no `FLUIDS` capability, and packagers and
  frogports offer no `ITEMS` handlers to hoppers, funnels and chutes. A close/abort callback on `Transaction`
  (or a journal for mod state) would close both.
- **A notice for any container's contents changing.** `contents_changed` reaches only the changed block's own
  parts, so a chute under a vanilla container and a frogport above an inventory re-check on a timer instead.
- **An empty stack from `menu_quick_move`**: the clicked slot keeps what is left of `stack`, and an
  `ItemStack` is never empty, so a part cannot move a whole stack to a slot of its choosing (the
  schematicannon's gunpowder slot); an `Option<ItemStack>` there would close it.
- **Per-slot insert rules on `ModContainer`**: "insert only while empty" (depot, ejector, crafter, chute) and
  the toolbox's one item kind per compartment.
- **Enchantment maximum levels on the server** (the `max_enchanted` filter attribute).
- **`ConfigFiles::remove`** (deleting the oldest schematic upload), and a mod reaching the game folder's
  `schematics/` (uploads live in the mod's own data folder).

### Entities and moving structures

- **Moving colliders**: players cannot stand on, or be pushed by, bearings' and pistons' structures, trains or
  minecart contraptions (`notes/moving-structures-note.md`).
- **Mod entity types that are not template copies**, rideable by players: Create's seat entity and carriage seats
  are stood in by tagged item display entities; riding a moving structure's seats on bearings and pistons and
  clicking its blocks are out.
- **Reading the client level's light** (`LevelRef::brightness` answers on the logical server only): a
  moving structure's blocks are lit with the light the server read where they were last sent from,
  not where they are each frame (`ContraptionRenderer`'s per-block world light).
- **Display entities joining a level from a save**: `entity_join_level` is not told of them and
  `LevelRef::entities_in` does not list them, so the trains' block displays and seat stand-ins that come back
  with a chunk are found by a `kill` selector on a per-opening tag. Moving structures anchored to a block entity
  are drawn by it, lit with the anchor's light (no per-block light in a block entity's draw); trains and cart
  contraptions are drawn as vanilla block displays the server moves every tick.
- **`EntityRef::move_by` for any entity** (`Entity.move`): belts carry mobs by setting their motion
  each tick, and players by a push from their own client (`push_player`), where Create moves them.
- **`MinecartRef::set_carrying`** (a cart with a contraption counting as ridden) and **`EntityRef::move_by`**
  (moving a coupled cart off its rail with collisions).

### Client

- **Setting the local player's air on its client** (or the server re-sending an unchanged air supply): the
  diving helmet keeps a diver's air full on the server, but the client's bubbles still run down.
- **Changing a worn stack's data in place** (`ItemStack` mutation without `setItemSlot`): a backtank's air,
  written each second it is used, plays the armour's equip sound each time.
- **A placement answering another block's state**: `state_for_placement` ignores a state of another block,
  so a shaft placed in front of a steam engine becomes its powered shaft a tick later
  (`ShaftBlock.pickCorrectShaftType`), from any placement, not only a player's.
- **An item's own renderer** (`BlockEntityWithoutLevelRenderer`): the extendo grip is drawn as its plain item
  model, not Create's extending arm.
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
- **The player in `left_click_block`**: an arm's chosen point and an ejector's target can't be dropped
  by left-clicking them with the item.
- **Retexturing a block's terrain model** (Create's `BeltModel`): a belt with a brass casing shows the
  andesite casing's texture.
- **A neighbour's light on the client**: a filter in a smart chute's slot is drawn fully lit, as the
  light inside the chute is dark.
- **A global hit hook naming the attacker** (an extendo grip in the off hand wearing on hits).
- **Translations on the server** (`hover_name`, `translate`): display links use Create's en_us text.

## Left out of the port, not for want of API

- Logistics: a sloped belt collides as a flat one; Create's `BeltShapes` (by facing, slope and part) are
  not declared yet, though content shapes can select by several properties (as the steam engine's do).
- Fluids: the smart pipe's filter is set by a click anywhere on the pipe, not only on its filter slot;
  potion fluids (`PotionFluid`, potions in spouts, drains and tanks) and builder's tea.

## Not to send

- The earlier port's `feat/create-hooks` branch on OlaHulleberg/horizon added `native_*` hook points for the
  mixin bundle. The fork's API replaced that route; none of it is needed.
