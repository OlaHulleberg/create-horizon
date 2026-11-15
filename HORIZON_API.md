# What the port needs from the mod API

The mod runs on Sunrise ([sunrise-org/sunrise](https://github.com/sunrise-org/sunrise), the successor of the
arismoko/horizon fork, whose mod API it carries unchanged) and targets its mod API at the build
`create/horizon-mod.toml` pins (`b77574eb1b`). The gaps below were found porting at the fork's `42c5f703e3`;
every subsystem of the earlier mixin-based port is on the API, and this page lists what the API could not
express, so those behaviours are left out (each module's doc says the same where it applies). Some are closed
or narrowed by the newer API and not taken up yet: re-check a line against the pinned SDK before acting on it
(`cargo horizon-mod sdk diff <old> <new>` lists what changed).

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
  Packagers and the redstone requester moved to a solid template (they need no waterlogging), frogports
  and postboxes to waxed copper grate's (solid and waterloggable, letting light through), so signs stay on
  all of them as in Create; a block needing waterlogging, a solid face and the template of another class
  has no such choice. Computing `isSolid` from the declared collision shape (or a `solid` declaration
  field) would close it.
- **A block state's light emission and friction** (`BlockState.getLightEmission`, `getFriction`):
  copycats sound as their material (the `sound_type` part, with `HostBlockStates::sound`), but can't glow or
  slide as it does (`CopycatBlock.getLightEmission`, `getFriction`): the `light_emission` and `friction` parts
  would answer per copycat, but nothing reads a material state's light or friction. Two `HostBlockStates`
  facts, `light_emission(state) -> Option<u8>` and `friction(state) -> Option<f32>`, would close it.
- **Block-state facts:** `isRedstoneConductor` and `isSolidRender` (cart assemblers, stock links, schematic
  export approximate them by "full collision cube"); `requiresCorrectToolForDrops` / `isCorrectToolForDrops`
  (deployers dig every block at its harvestable speed: `can_harvest_block` and `player_harvest_check` hear the
  game's own dig and break, but a deployer counts its progress itself, before any break); the weathering-copper class and age; a block's class
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

- **Crafting recipes in `recipes_of_type`**: at the pinned build `recipes_of_type("minecraft:crafting")`
  lists no recipe (nor any `minecraft:crafting_*` serializer id, the key the earlier build used), though
  smelting, stonecutting, smithing and the mod's own types list and the game crafts. So shapeless crafting
  recipes don't mix in a basin (`MechanicalMixerBlockEntity.matchStaticFilters`) and the factory gauge finds
  no crafting recipe for its output; the glue reads them as the API documents (`recipes::crafting_of`), so
  they come back once the game lists them.

### Containers, items and transactions

- **Rolling back mod-owned state with a transaction.** A handler cannot learn that its transaction aborted,
  so Create's tanks (whose contents live in the engine) offer no `FLUIDS` capability, packagers and
  frogports offer no `ITEMS` handlers to hoppers, funnels and chutes, nor do belts (`ItemHandlerBeltSegment`;
  so a tunnel offers none of its belt's: an andesite tunnel none at all, a brass tunnel only its own stack to
  insert into, nothing to take out), nor do upward saws (`SawBlockEntity`'s inventory: hoppers can't feed one), and
  a portable storage interface
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
- **A fluid, or a model with a sprite swapped, drawn by a display entity; or an entity renderer with the
  client's host**: on trains and cart contraptions (block displays the server moves) what block entities hold
  is drawn by item, text and block displays riding with them (`glue/trains/contents.rs`), but a tank's,
  basin's, spout's or drain's fluid has no display to be (block displays draw block models, fluids have
  none), a dyed belt's halves and a cased belt's casing are their models with a sprite swapped
  (`TerrainModelOptions`), which a block display cannot ask, and a wall banner's model has no item for an item
  display (it is drawn as the standing one, its pole included). `EntityRenderer::submit` (which could draw
  them with `fluid_box` and `terrain_model`) is offered only for thrown and living types and has no
  `ClientHost` to read what the entity carries. A fluid display (a fluid stack and a box), or a display type a
  mod draws with `BlockEntityDrawContext` and its host, would close it.
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
- **The local player's swing on its client** (`RenderHandEvent.getSwingProgress`): the extendo grip's arms
  reach out in a first-person hand as Create's (`ExtendoGripRenderHandler`), started by a raw click in the
  world (`mouse_button_post`), as nothing on the client tells a swing (`attack_strength_scale` is the
  server's); a swing started otherwise (a held attack, a key) doesn't move them. Its poses are picked by
  `item_model_update` from ones `create-data` lists (the cog in tenths of its 45° repeat, the swing in eight
  steps), by game ticks, as the node is told no partial tick.
- **Whether an item's model is 3D** (`BakedModel.isGui3d`): belts, depots, placards and link slots approximate
  it by "the item places a block".
- **Keeping only some of a block entity's drawing**: `keep()` keeps everything drawn for it, so a block with
  one animated part is drawn every frame (the glue decides through `Subsystem::animated`). A way to hide
  another part's model would let the clockwork bearing's hour hand replace its top.
- **A colour over a block model a block entity draws** (`SuperByteBuffer.color`): an overstressed
  network's turning parts flash red, and a recovered one's green, fading (`KineticEffectHandler
  .overStressedEffect`); `terrain_model`'s tint reaches only quads with a tint index, so every kinetic part
  would need a tinted copy (`create-data` makes them for the arm, the gauge's arrows and the station's
  assembly overlay), and the flash is left out. The smoke and cloud that go with it are played.
- **A train station's signal and observer overlays' facing** is not an API gap but not kept: the port keeps
  no `TrackTargetingBehaviour.targetDirection`, so signals' and observers' track markers are not drawn
  (stations draw theirs, turned by their last assembly direction).
- **Collision shapes in `can_survive`** (`LevelHost` has none): a hand crank or valve handle is refused
  only against air or a fluid as it is placed, where Create asks for any collision shape behind it (the
  neighbour check after placement reads them through `LevelRef::collision_boxes`).
- **Text alpha in `BlockEntityDrawContext::text`** (display boards' 0.85 / 0.975).
- **The local player's ground flag and swimming on its client**: the diving boots sink, jump and stride as
  Create's (`player_tick_pre`, `movement_input`, `push_player`), but the client can't read its own player's
  `onGround`/`verticalCollision` (`EntityRef::on_ground` is the server's; a block shape just under or over the
  player's box stands in), and nothing keeps it from swimming (`HeavyBootsOnPlayerMixin`: `isUnderWater` false).
- **Consuming a key in `input_key`** (driving a train).
- **A sound at a position for everyone but one player** (`Level.playSound(player, ..)`): the cardboard sword's
  bonk on a block is played from the server to every player near it, the clicker included, where Create's
  client plays it at once and the server leaves the clicker out (`LevelRef::play_sound` has no exclusion and
  `EntityRef::play_sound` plays at the entity); a client-side left click itself is seen by `mouse_button_pre`
  with `crosshair_target`.
- **A terrain model that reads its neighbours** (Create's `CTModel` and `CTSpriteShifter`, connected textures):
  casings, framed glass, windows, layered stone, pillars, weathered iron windows, scaffolding, chassis, girder
  poles, trapdoors, vaults and encased blocks join through the OptiFine connected-texture rules `create-data
  prepare` writes, and fluid and creative tanks join as far as their states tell (up and down by `top` and
  `bottom`, a two-wide windowed tank's sides by its corner `shape`); but a rule joins by block or by block
  state only: an encased block never joins its casing (`CasingConnectivity`), a vault joins a neighbouring
  vault in the same state, and joins decided by a block entity or by collision heights are left out (plain
  tanks two or three wide and three-wide windowed ones keep a seam between their columns; crafters, brass
  tunnels' tops, copper roofs). `add_section_geometry` reads the neighbouring states and could mesh the joins
  decided by states (an encased block and its casing) as the mod's own quads, over a block model the mod
  would have to leave out; it is given no block entity data, so the vault, tank and crafter joins stay out.
  A sprite shift per face that reads neighbouring states and block entity data would close it.
- **The level's redstone in `state_for_placement`** (`LevelHost` has no `has_neighbor_signal`): a clutch,
  gearshift or adjustable chain gearshift placed beside power is placed unpowered and takes `powered` in the
  same tick's level tick (`GearshiftBlock.getStateForPlacement` reads it at once).
- **A neighbour's light on the client**: a filter in a smart chute's slot is drawn fully lit, as the
  light inside the chute is dark.
- **Hanging entities in a box** (`Level.getEntitiesOfClass` over an `AABB`): `LevelRef::entities_in` leaves
  item frames and paintings out, so a schematic saved by the port (`StructureTemplate.fillEntityList`) holds
  none; one that holds them prints them. Listing them in `entities_in` (or a lookup of its own) would close it.
- **Calling a block's `setPlacedBy` from code without a player**: the schematicannon writes a door's, tall
  plant's and bed's other half itself as those blocks' `setPlacedBy` does (`glue/schematics`), where Create
  calls the block's own (`BlockHelper.placeSchematicBlock`, flags `CLIENTS | KNOWN_SHAPE`, no placer).
  `PlayerRef::place_block` runs `setPlacedBy`, but only for an online player standing in the cannon's level
  and with `NEIGHBORS` added to the write; a `LevelRef` placement running `setPlacedBy` with the caller's
  flags would close it (and reach other mods' two-block blocks).
- **The local player's inventory slots on its client** (`player_inventory_slots` answers on the server only;
  `player_inventory` on a client lists stacks without their slots): the wand of symmetry's mirrors, drawn for
  every wand in the hotbar (`SymmetryHandler.onRenderWorld`), are sent to the client by the server.
- **A turned item in a screen** (`GuiGameElement.of(stack).rotate(..)`): the wand of symmetry's screen draws the
  wand flat.
- **Marking an entity immune to fire** (`Entity.fireImmune`, Create's `CreateFireImmune`): a player in a whole
  netherite diving set takes no fire damage (`living_incoming_damage`) and stops burning each tick, and their
  own view shows no flames, but lava still sets them alight for a tick, so others see flames on them.
- **Moving a playing sound** (a tickable `SoundInstance`'s `getX/Y/Z`, read each tick): turning machinery's
  ambient hum and cog clicks (`SoundScapes`, `ContinuousSound`) follow the mean of their blocks in Create; a
  `SoundRef` can change its volume (`fade`) but not its position, so they play at the listener with the volume
  their distance gives them, from no side (`glue/sources/ambience.rs`). A `SoundRef::move_to(position)` would
  close it.
- **Light at a position on the client**: the steam jets (`SteamJetParticle`) are drawn as Create's four
  turned quads through the glue's world pipeline, but a pipeline is unlit and the client has no light read
  (`LevelRef::brightness` is the server's), so a jet is full bright where Create's takes the light at its
  position; as world quads they are not sorted among the particles.
- **A tinted particle with a block sprite and an exact velocity** (Create's `FluidStackParticle`: the fluid's
  still sprite, its tint times 0.8, the spawn's velocity, gravity and collision): the item particle
  (`ParticleOptions::item`) moves as Create's does but is drawn untinted, and the block particle takes a tint
  but its constructor randomises the velocity (`TerrainParticle`), so pipes pour and drip milk, tea, honey,
  chocolate and lava as Create does, and water and potions in their colour but scattered (`glue/fluids/flows.rs`).
  A mod particle can take the exact velocity and a tint (`ModParticle`), but its sprite set's names resolve
  under `textures/particle/` (`<ns>:particle/<path>`), so it can't name a fluid's block texture, vanilla
  water's and lava's least of all. A tint on `ParticleOptions::item` (`rCol/gCol/bCol`) would close it.
- **A screen layout for a mod menu opened without a block** (`player_open_mod_menu` with no `pos`: the
  client's `open_native_screen` returns without a `Screen`): the filters' and the schedule's screens are the mod's own screen,
  and clicking an inventory slot carries a ghost copy, not the stack (Create's menus carry the real one), nor
  does a creative middle click put a stack of a ghost slot on the cursor. `PlayerRef::back_mod_menu_slots`
  now backs such a menu's slots with real storage on the server, but the client still has no container
  screen to click them through. A `Screen` for a menu type (not only a block entity's), or a `screen` part
  hook keyed by menu, would close it.
- **An item's tooltip lines**: the filters' screens show a slot's name, not its tooltip. (An item's tags are
  found by testing the tags a data reload listener lists under `tags/item`.)
- **A block state's model drawn in the world without a block entity** (`SchematicRenderer`'s ghost blocks):
  a deployed schematic is outlined, not drawn; `draw_model_instances` takes only registered standalone models,
  not any block state's. The placement helpers' preview is the same want, translucent: catnip's
  `PlacementHelpers.tick` shows the block a click would place as a ghost (`GhostBlocks.showGhostState(..)
  .breathingAlpha()`), for the shaft and cog helpers, copycats, girders, table cloths, sails, ladders and
  display boards; a `WorldDrawContext` call drawing a `BlockState` at a position with an alpha, for one frame,
  would close both (the helpers themselves place as Create's do).
- **Items in the world outside a block entity**: a value box's number is drawn by its block's block entity
  (creative motors, speed controllers, valve handles, brass diodes) and the option boxes' icons and frames
  (`IconValueBox`, `VALUE_BOX_HOVER`) as world quads of the icon atlas, but the filter slots' items and
  counts (`FilteringRenderer`) are left out: a world-draw hook can draw neither an item nor text. A kept
  block entity is drawn anew only when its data changes, so the server resends a looked-at block's entity
  with a look count.
- **A slot holding more than 99**: a toolbox compartment holds 4 stacks (256); its screen draws the
  compartment's total over its one slot after the screen is drawn, but the other three stacks are filled by the
  deposit button and hoppers, not by clicks.
- **Translations on the server** (`hover_name`, `translate`): display links use Create's en_us text.

## Left out of the port, not for want of API

- Decor: the placement helpers' indicator round the crosshair (catnip's `PlacementHelpers.afterRenderOverlay`,
  arrows toward where the click places): catnip's source is not among the port's references. A shop's
  overlay doesn't cycle the tooltips of what it sells (`renderComponentTooltip`), and a seated keeper's
  overlay of a shopping list shows for any list (Create's client also matches the ticker's network, which its
  client doesn't have here).
- Contraptions: contraption controls on a bearing's structure are not carried (their setting doesn't turn its
  actors off, `ContraptionControlsMovement.startMoving`), and a bearing's glue stays in the world while it turns,
  going back turned with it; an elevator's contraption controls choose no floor (`ElevatorControlsHandler`, the
  floor list and `ElevatorTargetFloorPacket`).
- Equipment: a haunted bell's soul pulse showing where mobs may spawn (`HauntedBellPulser`, `SoulPulseEffect`)
  and the soul particles and sounds after it rings.
- Trains: a track's paving with an off-hand block other than metal girders (`TrackPaver`: slabs, full
  blocks and walls under a placement; girder curves pave and pay as Create's).

## Not to send

- The earlier port's `feat/create-hooks` branch on OlaHulleberg/horizon added `native_*` hook points for the
  mixin bundle. The fork's API replaced that route; none of it is needed.
