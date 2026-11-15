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
- **Reading and writing the scoreboard** (`Scoreboard.getObjective`, `listPlayerScores`, `addObjective`,
  `getOrCreatePlayerScore(..).set`): Create's scoreboard and death count display sources
  (`ScoreboardDisplaySource`, `DeathCounterDisplaySource`) are left out (`glue/display`). A player's deaths
  are readable (`player_stat`), but the death counter keeps them in its own objective (`create_auto_deaths`),
  whose scores outlive a player going offline and show in `/scoreboard`; a host call listing an objective's
  scores (owner and value) by its name, and one adding a dummy objective and setting a holder's score, would
  close both.
- **A world-open phase.** `world_lifecycle` reports only failures and closing; Create's world opens at the
  first `level_load`.
- **`isSolid` (legacy solidity) of a declared shape**: a declared block keeps its template's `isSolid`
  (`BlockBehaviour.calculateSolid` is not run on the declared shapes), so a sign, banner or torch on a wall
  (`WallSignBlock.canSurvive`) drops off a block on the heavy core's template, however full its shape.
  Packagers and the redstone requester moved to a solid template (they need no waterlogging); frogports
  and postboxes, waterlogged on the heavy core, still drop a sign put on them. Computing `isSolid` from the
  declared collision shape (or a `solid` declaration field) would close it.
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

- **A fake player's attack strength** (`DeployerFakePlayer.getCurrentItemAttackStrengthDelay`, 1/64, so
  every blow lands charged): `PlayerRef::perform_action_scoped` with `PlayerAction::Attack` hits with the
  fake's own uncharged ticker, so a deployer's punch deals about a fifth of Create's damage (0.21 of a
  fist's 1). A `PlayerActionSetup` field for the attack strength (or a full charge) would close it.

- **A native mod's enchantments in the item system**: Create's `data/create/enchantment/capacity.json` and
  `potato_recovery.json` ship with the mod (`create-data prepare`) and commands and item components take them
  (`/give @s create:copper_backtank[enchantments={"create:capacity":3}]`, read by the glue), but the game's
  item enchantment table (`horizon-items`' `Enchantments`, loaded from the game's root and world packs) does not
  hold them: `/enchant` answers "cannot support that enchantment", tooltips leave them out and enchanting tables
  and anvils never offer them. Loading a native mod's `data/<ns>/enchantment` into that table would close it.
- **A mob's own saved data and a conversion keeping it**: a horse in a haunting fan current turns into a skeleton
  horse after 100 ticks (`HauntingType.affectEntity`, its progress in the horse's persistent data, then a
  skeleton horse made from the horse's saved data); no API keeps a counter on a mob nor converts one keeping its
  data, so haunted horses are only blinded and slowed.
- **`Block.fallOn` as a block part hook** (or the block landed on in `living_fall`, which is given no level):
  a seat halves the fall it breaks (`SeatBlock.fallOn`); its bounce is ported (`bounce_restitution`).
- **A native mod's damage types**: Create's `data/create/damage_type/*.json` (`crush`, `mechanical_saw`,
  `fan_fire`...) shipped with the mod are not registered (`EntityRef::hurt` with `create:crush` is refused; with
  the files shipped, `/damage` parses `create:crush` but the health code's damage table answers "invulnerable"),
  so crushing wheels hurt with vanilla's `cramming`, which also goes through armour, saws and drills with
  `generic`, and fans with `lava` (blasting) and `in_fire` (smoking).
- **Moving colliders**: players cannot stand on, or be pushed by, bearings' and pistons' structures, trains or
  minecart contraptions (`notes/moving-structures-note.md`).
- **Mod entity types that are not template copies**, rideable by players: Create's seat entity and carriage seats
  are stood in by tagged item display entities; riding a moving structure's seats on bearings and pistons and
  clicking its blocks are out (contraption controls on a gliding structure are the exception: the client
  raycasts the structures it draws, as `ContraptionHandlerClient` does, and sends the click).
- **A block entity type for a block by its name**: the game gives `create:mechanical_piston_head` a
  `minecraft:skull` block entity (its path ends in `_head`), and one is left behind on the block that takes the
  head's place when a piston retracts. A declaration field naming a block's entity type (or none) would close it.
- **Reading the client level's light** (`LevelRef::brightness` answers on the logical server only): a
  moving structure's blocks are lit with the light the server read where they were last sent from,
  not where they are each frame (`ContraptionRenderer`'s per-block world light).
- **Display entities joining a level from a save**: `entity_join_level` is not told of them and
  `LevelRef::entities_in` does not list them, so the trains' block displays and seat stand-ins that come back
  with a chunk are found by a `kill` selector on a per-opening tag. Moving structures anchored to a block entity
  are drawn by it, lit with the anchor's light (no per-block light in a block entity's draw); trains and cart
  contraptions are drawn as vanilla block displays the server moves every tick.
- **`EntityRef::move_by` for any entity** (`Entity.move`): belts carry mobs by setting their motion
  each tick, and players by a push from their own client (`push_player`), where Create moves them; up a
  slope they climb `BeltShapes`' stairs by stepping up, where Create moves them down 0.13 and across with a
  step height of 1.
- **A collision shape for an entity riding the block** (`BeltBlock.getCollisionShape`: the whole outline for
  a belt's passengers, cut at 19 px for others): content shapes select by entity class only, so every entity
  collides with a belt's cut shape.
- **`MinecartRef::set_carrying`** (a cart with a contraption counting as ridden: the glue gives back the
  drag an empty cart's rails take, but not the three-quarter step a ridden cart moves) and
  **`EntityRef::move_by`** (moving a coupled cart off its rail with collisions). `MinecartRef::is_on_rails`
  answered false for a cart running on a rail when read in a level tick, so the glue looks at the rail itself.
- **A mod's own projectile entity type with its own class** (Create's `create:potato_projectile`,
  with its damage type `create:potato_cannon`): the potato cannon's shots are vanilla snowballs
  carrying the ammunition, flown and hit by the glue, hurting as `minecraft:thrown` (a declared copy of
  the snowball draws as the item it is bound to, not the ammunition each shot carries). Their flight
  (gravity, drag, stuck to a target) is the glue's and is not saved: a shot in the air when the
  world is saved comes back as a plain snowball. A way to read or keep a projectile's state
  across a save (persistent data on `EntityRef`) would close it.
- **A fluid drawn with a display entity, or an entity renderer with the client's host**: on trains and cart
  contraptions (block displays the server moves) what block entities hold is drawn by item and text displays
  riding with them, but a tank's, basin's, spout's or drain's fluid has no display to be (block displays draw
  block models, fluids have none), and `EntityRenderer::submit` (which could draw it with `fluid_box`) is offered
  only for thrown and living types and has no `ClientHost` to read what the entity carries. A fluid display
  (a fluid stack and a box) or a display type a mod draws with `BlockEntityDrawContext` and its host would close it.
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
- **A special item model told its display context and the time** (`SpecialModelRenderer` is asked once per
  stack): the extendo grip is drawn with its arms, hand and cog at Create's resting pose everywhere, its cog
  still and its arms not reaching out in a first-person hand as the player swings
  (`ExtendoGripItemRenderer`, `ExtendoGripRenderHandler`).
- **Whether an item's model is 3D** (`BakedModel.isGui3d`): belts, depots, placards and link slots approximate
  it by "the item places a block".
- **Keeping only some of a block entity's drawing**: `keep()` keeps everything drawn for it, so a block with
  one animated part is drawn every frame (the glue decides through `Subsystem::animated`). A way to hide
  another part's model would let the clockwork bearing's hour hand replace its top.
- **A colour over a block model a block entity draws** (`SuperByteBuffer.color`): an overstressed
  network's turning parts flash red, and a recovered one's green (`KineticEffectHandler.overStressedEffect`);
  `BlockEntityDrawContext::block` draws a model in its own tints, and `terrain_model`'s tint reaches only
  quads with a tint index. The smoke and cloud that go with the flash are played. The display link's bulb
  flash (`LinkBulbRenderer`: its glow model drawn additively in a fading grey) is left out for the same
  want (and of an additive render type). A station's assembly track (`StationRenderer`: blue where a
  bogey may go, green where one is) is drawn in the overlay model's own colours for the same reason.
- **A train station's signal and observer overlays' facing** is not an API gap but not kept: the port keeps
  no `TrackTargetingBehaviour.targetDirection`, so signals' and observers' track markers are not drawn
  (stations draw theirs, turned by their last assembly direction).
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
  casings, framed glass, windows, layered stone, pillars, weathered iron windows, scaffolding, chassis, girder
  poles, trapdoors, vaults and encased blocks join through the OptiFine connected-texture rules `create-data
  prepare` writes, and fluid and creative tanks join as far as their states tell (up and down by `top` and
  `bottom`, a two-wide windowed tank's sides by its corner `shape`); but a rule joins by block or by block
  state only: an encased block never joins its casing (`CasingConnectivity`), a vault joins a neighbouring
  vault in the same state, and joins decided by a block entity or by collision heights are left out (plain
  tanks two or three wide and three-wide windowed ones keep a seam between their columns; crafters, brass
  tunnels' tops, copper roofs). A model hook given the neighbouring states and block entity data (or a sprite
  shift per face) would close it.
- **The level's redstone in `state_for_placement`** (`LevelHost` has no `has_neighbor_signal`): a clutch,
  gearshift or adjustable chain gearshift placed beside power is placed unpowered and takes `powered` in the
  same tick's level tick (`GearshiftBlock.getStateForPlacement` reads it at once).
- **A neighbour's light on the client**: a filter in a smart chute's slot is drawn fully lit, as the
  light inside the chute is dark.
- **The local player's inventory slots on its client** (`player_inventory_slots` answers on the server only;
  `player_inventory` on a client lists stacks without their slots): the wand of symmetry's mirrors, drawn for
  every wand in the hotbar (`SymmetryHandler.onRenderWorld`), are sent to the client by the server.
- **A turned item in a screen** (`GuiGameElement.of(stack).rotate(..)`): the wand of symmetry's screen draws the
  wand flat.
- **A global hit hook naming the attacker** (an extendo grip in the off hand wearing on hits).
- **Marking an entity immune to fire** (`Entity.fireImmune`, Create's `CreateFireImmune`): a player in a whole
  netherite diving set takes no fire damage (`living_incoming_damage`) and stops burning each tick, and their
  own view shows no flames, but lava still sets them alight for a tick, so others see flames on them.
- **Moving a playing sound** (a tickable `SoundInstance`'s `getX/Y/Z`, read each tick): turning machinery's
  ambient hum and cog clicks (`SoundScapes`, `ContinuousSound`) follow the mean of their blocks in Create; a
  `SoundRef` can change its volume (`fade`) but not its position, so they play at the listener with the volume
  their distance gives them, from no side (`glue/sources/ambience.rs`). A `SoundRef::move_to(position)` would
  close it.
- **A particle drawn as its own quads** (`SteamJetParticle.render`'s four quads turned along the jet): steam
  engines' and whistles' steam jets are Create's sprites on a billboard.
- **A tinted particle with a block sprite and an exact velocity** (Create's `FluidStackParticle`: the fluid's
  still sprite, its tint times 0.8, the spawn's velocity, gravity and collision): the item particle
  (`ParticleOptions::item`) moves as Create's does but is drawn untinted, and the block particle takes a tint
  but its constructor randomises the velocity (`TerrainParticle`), so pipes pour and drip milk, tea, honey,
  chocolate and lava as Create does, and water and potions in their colour but scattered (`glue/fluids/flows.rs`).
  A tint on `ParticleOptions::item` (`rCol/gCol/bCol`) would close it.
- **A screen layout for a mod menu opened without a block** (`player_open_mod_menu` with no `pos`: the
  client's `open_native_screen` returns without a `Screen`): the filters' and the schedule's screens are the mod's own screen,
  and clicking an inventory slot carries a ghost copy, not the stack (Create's menus carry the real one). A
  `Screen` for a menu type (not only a block entity's), or a `screen` part hook keyed by menu, would close it.
- **An item's tooltip lines**: the filters' screens show a slot's name, not its tooltip. (An item's tags are
  found by testing the tags a data reload listener lists under `tags/item`.)
- **A block state's model drawn in the world without a block entity** (`SchematicRenderer`'s ghost blocks):
  a deployed schematic is outlined, not drawn; `draw_model_instances` takes only registered standalone models,
  not any block state's.
- **GUI sprites in a block entity's draw**, and **text and sprites on a block face in the world** outside one:
  a value box's number is drawn by its block's block entity (creative motors, speed controllers, valve
  handles, brass diodes), but the icon boxes (`IconValueBox`: movement and selection modes) and the filter
  slots' items and counts (`FilteringRenderer`) are left out. A kept block entity is drawn anew only when its
  data changes, so the server resends a looked-at block's entity with a look count.
- **A slot holding more than 99**: a toolbox compartment holds 4 stacks (256); its screen draws the
  compartment's total over its one slot after the screen is drawn, but the other three stacks are filled by the
  deposit button and hoppers, not by clicks.
- **A clip rectangle in `DrawContext`** (`GuiGraphics.enableScissor`): the schedule's cards scrolled past its
  panel are covered by the panel's frame drawn again, and a card's condition groups that don't fit across it are
  left out until scrolled to, where Create cuts them at the card's edge.
  The stock keeper's list and the stock ticker's category cards are drawn a whole row or card at a time: one
  scrolled partly out of the window is left out where Create shows the part inside it.
- **A live entity drawn in a screen** (`InventoryScreen.renderEntityInInventoryFollowsMouse`): the stock keeper's
  screen draws a blaze keeper (its burner as a scene block, `DrawContext::draw_scene`), but not a seated mob
  keeper beside the window. A `DrawContext` call drawing an `EntityRef` at a size, looking at a point, would
  close it.
- **Translations on the server** (`hover_name`, `translate`): display links use Create's en_us text.

## Left out of the port, not for want of API

- Display and redstone: the clipboard copies no steam engine's `RotationDirection` and no table cloth's price,
  neither setting being ported; redstone links feeding factory gauges (`RedstoneLinkBlockEntity`'s
  `FactoryPanelSupportBehaviour`) are not joined (display links are).
- Contraptions: trains and cart contraptions, drawn as block displays, show what their block entities hold
  through item and text displays riding with them for depots' and ejectors' stacks, nixie tubes' and signs' text
  (`glue/trains/contents.rs`); belts' and basins' items, chutes' stacks, display boards' text, banners' patterns
  and crafters' grids are not shown while they ride (their fluids: see the gap above).
- Redstone: smart observers and threshold switches read a fluid only from a fluid tank (Create reads any fluid
  handler: basins, drains, spouts, and an observer the flow through a pipe), and ignore packages on a chain
  conveyor; a nixie tube on a train signal shows its text, not the signal's lights (`NixieTubeRenderer`); a
  redstone contact on an elevator column is not turned into an elevator contact by the wrench; dyed items
  (`DYED_COLOR`) make one link frequency with their undyed item.
- Processing: a deployer does not feed food or drinks to a player in front of it (`finishUsingItem` on another
  player), and its overflow (`overflowItems`) drops at it rather than staying for funnels and hoppers; crafting
  remainders of saw, crushing and millstone inputs are not given back.
- Decor: table cloths hold no items and run no shops (`TableClothBlockEntity`); copycats have no placement
  helpers and do not take their material's sound, friction or light; girders have no placement helper; an item
  hatch's filter compares item ids, not a filter item's list or attributes.
- Equipment: the toolbox radial menu and picking a toolbox up with its contents, the linked controller's
  frequency screen, the extendo grip's knockback and entity reach, the worldshaper's flatten tool, the
  schematicannon's belts, entities and checklist books, and the second half of doors and beds it prints.

## Not to send

- The earlier port's `feat/create-hooks` branch on OlaHulleberg/horizon added `native_*` hook points for the
  mixin bundle. The fork's API replaced that route; none of it is needed.
