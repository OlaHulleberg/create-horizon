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
- **The server's tick freeze** (`TickRateManager.runsNormally`): under `/tick freeze` Create's block
  entities stop, but a mod's level tick still runs, so the glue's machines go on (`glue/redstone`).
- **Reading the scoreboard** (objectives and their scores): Create's scoreboard and death count display
  sources (`ScoreboardDisplaySource`, `DeathCounterDisplaySource`) are left out (`glue/display`).
- **A world-open phase.** `world_lifecycle` reports only failures and closing; Create's world opens at the
  first `level_load`.
- **Block-state facts:** `isRedstoneConductor` and `isSolidRender` (cart assemblers, stock links, schematic
  export approximate them by "full collision cube"); `requiresCorrectToolForDrops` / `isCorrectToolForDrops`
  (deployers dig every block at its harvestable speed); the weathering-copper class and age; a block's class
  (copycats guess stairs, bars and trapdoors from tags and ids).
- **A state's numeric id**, for `level_event(2001, ..)` (block-break particles on a wrenched copycat, a
  belt's casing taken off, an encased shaft's or cog's casing taken off with the wrench).
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
  limestone or scoria from flowing lava, `registerFluidInteractions`) have no declaration; the glue finds them
  when the lava or the honey next ticks (`fluid_tick`) and turns the lava in the level's next tick, where
  Create's happen at once on the neighbour's change.

### Containers, items and transactions

- **Rolling back mod-owned state with a transaction.** A handler cannot learn that its transaction aborted,
  so Create's tanks (whose contents live in the engine) offer no `FLUIDS` capability, packagers and
  frogports offer no `ITEMS` handlers to hoppers, funnels and chutes, and a portable storage interface
  offers its contraption's storage to Create's chutes and funnels only, not to hoppers. A close/abort callback
  on `Transaction` (or a journal for mod state) would close them.
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

- **A native mod's damage types**: Create's `data/create/damage_type/*.json` (`crush`, `mechanical_saw`,
  `fan_fire`...) shipped with the mod are not registered (`EntityRef::hurt` with `create:crush` is refused), so
  crushing wheels hurt with vanilla's `cramming`, which also goes through armour.
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
- **A mod's own projectile entity type with its own class** (Create's `create:potato_projectile`,
  with its damage type `create:potato_cannon`): the potato cannon's shots are vanilla snowballs
  carrying the ammunition, flown and hit by the glue, hurting as `minecraft:thrown` (a declared copy of
  the snowball draws as the item it is bound to, not the ammunition each shot carries). Their flight
  (gravity, drag, stuck to a target) is the glue's and is not saved: a shot in the air when the
  world is saved comes back as a plain snowball. A way to read or keep a projectile's state
  across a save (persistent data on `EntityRef`) would close it.
- **A zombie villager's cure from code** (`ZombieVillager.mobInteract` with a golden apple, or
  `startConverting`): a golden apple from the potato cannon gives its food effects instead.

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
- **A colour over a block model a block entity draws** (`SuperByteBuffer.color`): an overstressed
  network's turning parts flash red, and a recovered one's green (`KineticEffectHandler.overStressedEffect`);
  `BlockEntityDrawContext::block` draws a model in its own tints, and `terrain_model`'s tint reaches only
  quads with a tint index. The smoke and cloud that go with the flash are played.
- **Collision shapes in `can_survive`** (`LevelHost` has none): a hand crank or valve handle is refused
  only against air or a fluid as it is placed, where Create asks for any collision shape behind it (the
  neighbour check after placement reads them through `LevelRef::collision_boxes`).
- **Text alpha in `BlockEntityDrawContext::text`** (display boards' 0.85 / 0.975), and more than one colour per
  line in tooltips (goggles).
- **A mutable `ClientHost` in screen hooks**: the clipboard screen closes and sends its pages from the next
  client tick.
- **Reading and setting the local player's movement on the client** (diving boots sinking), **consuming a key
  in `input_key`** (driving a train), and **a client-side left-click-block hook** (the cardboard sword's bonk
  is played from the server).
- **The player in `left_click_block`**: an arm's chosen point and an ejector's target can't be dropped
  by left-clicking them with the item.
- **A terrain model that reads its neighbours** (Create's `CTModel` and `CTSpriteShifter`, connected textures):
  casings, framed glass, windows and layered stone join through the connected-texture rules `create-data
  prepare` writes, but gearboxes, encased shafts and cogs and fluid tanks wider than one keep each its own
  frame, where Create's `EncasedCTBehaviour`, `EncasedCogCTBehaviour` and `FluidTankCTBehaviour` join them
  into one (their models are not plain cubes a rule can retexture). A model hook given the neighbouring
  states (or a sprite shift per face) would close it.
- **The level's redstone in `state_for_placement`** (`LevelHost` has no `has_neighbor_signal`): a clutch,
  gearshift or adjustable chain gearshift placed beside power is placed unpowered and takes `powered` in the
  same tick's level tick (`GearshiftBlock.getStateForPlacement` reads it at once).
- **More than one UV shift in a `terrain_model` draw** (`TerrainModelOptions::uv_shift` takes one
  sprite's): item vaults' connected textures (`ItemVaultCTBehaviour` picks each face's tile of the
  front, side, top and bottom sheets by the vault's size) would be drawn by the vault's block entity
  with one shift per sprite; they are not drawn.
- **A neighbour's light on the client**: a filter in a smart chute's slot is drawn fully lit, as the
  light inside the chute is dark.
- **The local player's inventory slots on its client** (`player_inventory_slots` answers on the server only;
  `player_inventory` on a client lists stacks without their slots): the wand of symmetry's mirrors, drawn for
  every wand in the hotbar (`SymmetryHandler.onRenderWorld`), are sent to the client by the server.
- **A turned item in a screen** (`GuiGameElement.of(stack).rotate(..)`): the wand of symmetry's screen draws the
  wand flat.
- **A global hit hook naming the attacker** (an extendo grip in the off hand wearing on hits).
- **A screen layout for a mod menu opened without a block** (`player_open_mod_menu` with no `pos`: the
  client's `open_native_screen` returns without a `Screen`): the filters' and the schedule's screens are the mod's own screen,
  and clicking an inventory slot carries a ghost copy, not the stack (Create's menus carry the real one). A
  `Screen` for a menu type (not only a block entity's), or a `screen` part hook keyed by menu, would close it.
- **An item's tags** (`ItemStack.getTags`) and **an item's tooltip lines**: the attribute filter offers no
  `in_tag` attributes for its reference item, and the filters' screens show a slot's name, not its tooltip.
- **A block state's model drawn in the world without a block entity** (`SchematicRenderer`'s ghost blocks):
  a deployed schematic is outlined, not drawn; `draw_model_instances` takes only registered standalone models,
  not any block state's.
- **Text and sprites on a block face in the world** outside a block entity's draw: a value box's value and icon
  (`ValueBox.renderContents`) are left out; only its outline is drawn.
- **Block textures in `draw_scene`**: the wand of symmetry's mirror preview (a part block's model in a scene)
  shows nothing in game, while a toolbox's draws; the scene looks its sprites up in the GUI atlas
  (`native_scene.rs`), which likely lacks textures that no item uses. Not confirmed further.
- **A slot's count text and a slot holding more than 99**: a toolbox compartment holds 4 stacks (256), Create's
  screen shows the compartment's total on its one slot; here a slot shows its own stack, and the other three are
  filled by the deposit button and hoppers, not by clicks.
- **A clip rectangle in `DrawContext`** (`GuiGraphics.enableScissor`): the schedule's cards scrolled past its
  panel are covered by the panel's frame drawn again, and a card's condition groups that don't fit across it are
  left out until scrolled to, where Create cuts them at the card's edge.
- **Translations on the server** (`hover_name`, `translate`): display links use Create's en_us text.

## Left out of the port, not for want of API

- Logistics: a sloped belt collides as a flat one; Create's `BeltShapes` (by facing, slope and part) are
  not declared yet, though content shapes can select by several properties (as the steam engine's do).
- Contraptions: a deployer on a moving contraption does nothing (`DeployerMovementBehaviour`), and moving
  block entities other than mounted storage (chests, barrels, vaults, toolboxes) keep only their block (a
  depot's item, a deployer's held item, a tank's fluid are not carried).
- Fluids: creative fluid tanks stay one block each (Create's join into tanks up to 3 wide); potion fluids
  keep registered potions only (custom effects are lost), and belts carry only the potions a spout fills on
  them (the logistics glue keeps no components on belts); the boiler's gauge is drawn at its level without
  its swing; glass pipes show a steady stream, not its growth, and open pipe ends have no flow particles.
- Steam engines: their jets and the boiler's hiss come only from engines being drawn (the client's draw
  of the engine times them, not a client tick), and the hiss is at the volume of a boiler with up to
  eleven engines.

## Not to send

- The earlier port's `feat/create-hooks` branch on OlaHulleberg/horizon added `native_*` hook points for the
  mixin bundle. The fork's API replaced that route; none of it is needed.
