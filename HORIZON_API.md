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
- **A block's shape from its block entity** (`FactoryPanelBlock.getShape`, built from the gauge's active panels):
  shapes are declared per state, so a factory gauge's outline and collision are Create's
  `FACTORY_PANEL_FALLBACK`, the whole face, however few panels it has (`glue/stock/gauge.rs`). A shape hook
  for a block part (outline and collision, with the position) would close it.
- **Reading and writing the scoreboard** (`Scoreboard.getObjective`, `listPlayerScores`, `addObjective`,
  `getOrCreatePlayerScore(..).set`): Create's scoreboard and death count display sources
  (`ScoreboardDisplaySource`, `DeathCounterDisplaySource`) are left out (`glue/display`). A player's deaths
  are readable (`player_stat`), but the death counter keeps them in its own objective (`create_auto_deaths`),
  whose scores outlive a player going offline and show in `/scoreboard`. `Commands::run_command_with_output`
  reaches them only as console text (`scoreboard players list`, `.. get`) to be parsed in the server's
  language; a typed host call listing an objective's scores (owner and value) by its name, and one adding a
  dummy objective and setting a holder's score, would close both.
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
- **A state's numeric id**, for `level_event(2001, ..)`: a wrenched copycat, an encased shaft's or cog's
  casing, a belt's casing, an encased pipe's and chute's casing, a bracket taken off and the belt slicer's cut
  show the state's block particles (`LevelRef::send_particles`) and its break sound (`BlockStates::sound`),
  rebuilt in `ServerLevel::destroy_effect`, not the client's own `addDestroyBlockEffect` burst.
  `BlockStates::state_id` (or a 2001 call taking a `BlockState`) would make them exact.
- **The neighbour-shape update of a state placed by code** (`updateShape` against its neighbours): mirrored
  placements of the wand of symmetry skip it.
- **`OnBlockPlacedByEvent` without a level**: decor's fix-ups after a placement with no placer are skipped.
  `ConfigHost::dimension` may name the level there, unverified: no vanilla dispenser places these items.
- **Scaffolding's collision for a templated block**: Horizon gives `ScaffoldingBlock.getCollisionShape` (solid
  only from above, unless descending) and its dynamic shape only to `minecraft:scaffolding` by name; a copy
  without declared shapes has none (placing it fails, and an entity touching it stops the game). Create's metal
  scaffolding declares the stable shape, so it is stood on but not climbed through from inside; a declaration
  field (or class-based lookup) for scaffolding's context rule would close it.
- **Fluid interactions in content declarations**: honey and chocolate meeting lava (obsidian from a source,
  limestone or scoria from flowing lava, `registerFluidInteractions`) have no declaration; the glue finds them
  when the lava or the honey next ticks (`fluid_tick`) and turns the lava in the level's next tick, where
  Create's happen at once on the neighbour's change.

- **One recipe naming an unknown item empties its type for mods** (a game bug, found at `b77574eb1b`):
  `recipes_of_type` converts the type's whole list at once, and one recipe whose result (or a rule's stack)
  is an item the game doesn't know fails it, so the mod gets an empty list. The generated
  `impl RecipesHost for Game` (`game_host_modules.rs`, from the host generator) returns
  `Game::recipes_of_type(self, kind).try_into_api().unwrap_or_default()`;
  `TryIntoApi for Arc<[T]>` (`crates/horizon-client/src/game/modding/api_values.rs:573`) collects
  element by element with `?`, and `TryIntoApi for EngineStack` (`api_values.rs:733`) answers
  `StackReadError::UnknownItem` for an item with no `item_key`. `Recipes::from_resolved`
  (`crates/horizon-client/src/crafting.rs:1863-1898`) lists a shaped or shapeless recipe with
  `result.create()` without checking that the result's item exists. Create 6.0.10's
  `crafting/appliances/chain_from_zinc.json` names `minecraft:chain` (`minecraft:iron_chain` since 1.21.9),
  so `minecraft:crafting` gave 0 recipes where smelting and smithing listed. Repro: a world data pack with
  `data/repro/recipe/bad.json` = `{"type":"minecraft:crafting_shapeless","ingredients":["minecraft:stick"],"result":{"id":"minecraft:no_such_item"}}`;
  after `/reload` a mod's `recipes_of_type("minecraft:crafting")` gives 0 (1914 without the pack). Leaving
  out the recipes that fail (or failing the recipe at load) would fix it. The port renames the chain in its
  recipes (`tools/create-data`): the list is whole again (1914), a basin mixes shapeless recipes (mushroom
  stew, checked in game), and the factory gauge's crafting lookup reads the same list.

### Containers, items and transactions

- **Rolling back mod-owned state with a transaction.** A handler cannot learn that its transaction aborted,
  so Create's tanks (whose contents live in the engine) offer no `FLUIDS` capability, nor do belts offer
  `ITEMS` (`ItemHandlerBeltSegment`;
  so a tunnel offers none of its belt's: an andesite tunnel none at all, a brass tunnel only its own stack to
  insert into, nothing to take out), nor do upward saws (`SawBlockEntity`'s inventory: hoppers can't feed one), and
  a portable storage interface
  offers its contraption's storage to Create's chutes and funnels only, not to hoppers. A close/abort callback
  on `Transaction` (or a journal for mod state) would close them. Block entity slots, attachments and
  Part state (block entity and entity) are journaled, so moving these contents there is the route the API
  has. (Packagers and package ports mirror their
  boxes into block entity slots, which the game journals, and learn of automation's moves at their level's next
  tick, as a handler's own slot writes give no `contents_changed`: `glue/packages/automation.rs`.)
- **A notice for any container's contents changing.** `contents_changed` reaches only the changed block's own
  parts, so a chute under a vanilla container and a frogport above an inventory re-check on a timer instead.
- **An empty stack from `menu_quick_move`**: the clicked slot keeps what is left of `stack`, and an
  `ItemStack` is never empty, so a part cannot move a whole stack to a slot of its choosing (the
  schematicannon's gunpowder slot); an `Option<ItemStack>` there would close it.
- **Per-slot insert rules on `ModContainer`**: "insert only while empty" (depot, ejector, crafter, chute) and
  the toolbox's one item kind per compartment for hoppers and funnels (its menu keeps it, `menu_slot_rules`).
- **The block entity in `BlockDropsEvent`** (or its saved data through the event's `LevelHost`): a toolbox
  the level destroys with its drops (`/setblock .. destroy`, `Level.destroyBlock`) drops an empty toolbox,
  where Create's loot copies its contents (`copy_components` of `TOOLBOX_INVENTORY`); a player's break, an
  explosion and the port's own machines keep them, as the glue empties the toolbox first.
- **`ConfigFiles::remove`** (deleting the oldest schematic upload), and a mod reaching the game folder's
  `schematics/` (uploads live in the mod's own data folder).

### Entities and moving structures

- **A fake player's attack strength** (`DeployerFakePlayer.getCurrentItemAttackStrengthDelay`, 1/64, so
  every blow lands charged): `PlayerRef::perform_action_scoped` with `PlayerAction::Attack` hits with the
  fake's own uncharged ticker, so a deployer's punch deals about a fifth of Create's damage (0.21 of a
  fist's 1). A `PlayerActionSetup` field for the attack strength (or a full charge) would close it.

- **A native mod's enchantments in the item system**: at this pin `/enchant` applies Create's
  `create:capacity` to a backtank; whether tooltips, enchanting tables and anvils offer Capacity and Potato
  Recovery is not checked yet.
- **`Block.fallOn`'s distance**: a seat counts a fall at half its distance (`SeatBlock.fallOn`), but
  `FallOnEvent` lets a part change only the damage, so the glue works the halved fall's damage out from the
  game's (`seat::fall_damage`: vanilla's safe distance of 3, or the one the damage implies at a multiplier
  of 1). A writable `distance` would make it exact under other fall damage multipliers.
- **A native mod's damage types**: Create's `data/create/damage_type/*.json` (`crush`, `mechanical_saw`,
  `fan_fire`...) shipped with the mod are not registered (`EntityRef::hurt` with `create:crush` is refused; with
  the files shipped, `/damage` parses `create:crush` but the health code's damage table answers "invulnerable"),
  so crushing wheels hurt with vanilla's `cramming`, which also goes through armour, saws and drills (standing
  or moving, with moving rollers) and a train running a mob over (`run_over`) with `generic`, and fans with
  `lava` (blasting) and `in_fire` (smoking).
- **Moving colliders**: players cannot stand on, or be pushed by, bearings' and pistons' structures, trains or
  minecart contraptions (`notes/moving-structures-note.md`). A train still hurts and throws a grounded entity its
  blocks meet (`handleDamageFromTrain`, the glue's own overlap test, `glue/trains/actors.rs`), but the entity
  is not stopped by the carriage. `LevelRef::set_moving_colliders` (the audit's E6 proposal) would close it.
- **Mod entity types that are not template copies**, rideable by players: Create's seat entity and carriage seats
  are stood in by tagged item display entities; riding a moving structure's seats on bearings and pistons and
  clicking its blocks are out (contraption controls on a gliding structure are the exception: the client
  raycasts the structures it draws, as `ContraptionHandlerClient` does, and sends the click). Create's package
  entity (`PackageEntity`: a dropped package with its own model, physics and unpacking on a hit) is the same want:
  a dropped package is a plain item entity (`glue/packages`). A client can't ask an entity's vehicle
  (`EntityRef::vehicle` answers on the server only) nor find a seat's stand-in (`LevelRef::entities_in` leaves
  displays out), so a shop's checkout overlay takes a keeper as seated by where it is (`glue/stock/shop.rs`).
- **A block entity type for a block by its name** (a game bug at `b77574eb1b`): `create:mechanical_piston_head`
  (template `minecraft:andesite`, which has none) still gets a `minecraft:skull` block entity, against
  `behaviour:block_entity_templates`. `BlockEntity::for_block` (`crates/horizon-client-render/src/containers.rs:495`)
  falls back to the block's own name when `block_entity_types::factory_block` finds no template entry, and
  `for_factory` then matches `is_skull` (`containers.rs:371`, used at `:554`: any `_head` or `_skull`, in any
  namespace). Repro: a content block `mymod:test_head` on template `minecraft:andesite`, `/setblock ~ ~ ~
  mymod:test_head`, `/data get block ~ ~ ~` shows `id: "minecraft:skull"`. Checking the `minecraft:`
  namespace there would fix it.
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
  step height of 1. `Hooks::entity_move` can only change an entity's own move, takes no host and runs on the
  server only (a client does not predict it), so it is not that extra move.
- **A collision shape for an entity riding the block** (`BeltBlock.getCollisionShape`: the whole outline for
  a belt's passengers, cut at 19 px for others): content shapes select by entity class only, so every entity
  collides with a belt's cut shape.
- **`MinecartRef::set_carrying`** (a cart with a contraption counting as ridden: the glue gives back the
  drag an empty cart's rails take, but not the three-quarter step a ridden cart moves) and
  **`EntityRef::move_by`** (moving a coupled cart off its rail with collisions; `entity_move` replaces only
  the cart's own move, not the coupling's pull). `MinecartRef::is_on_rails`
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
- **A sprite scrolled within itself** (a UV offset that wraps inside the sprite): a chain conveyor's chains
  (`ChainConveyorRenderer.renderChain`, `minV` moving with the conveyor's speed) are drawn still, as vanilla's
  chain model a block at a time (`glue/packages`); `TerrainModelOptions` moves a sprite's UVs onto another sprite,
  which a texture that tiles on itself has none of.
- **A colour over a block model a block entity draws** (`SuperByteBuffer.color`): an overstressed
  network's turning parts flash red, and a recovered one's green (`KineticEffectHandler.overStressedEffect`);
  `BlockEntityDrawContext::block` draws a model in its own tints, and `terrain_model`'s tint reaches only
  quads with a tint index. The smoke and cloud that go with the flash are played. The display link's bulb
  flash (`LinkBulbRenderer`) draws its tube full bright in the translucent layer, but not the glow model
  drawn additively in a fading grey over it, for the same want (and of an additive render type). A station's assembly track (`StationRenderer`: blue where a
  bogey may go, green where one is) is drawn in the overlay model's own colours for the same reason.
- **Collision shapes in `can_survive`** (`LevelHost` has none): a hand crank or valve handle is refused
  only against air or a fluid as it is placed, where Create asks for any collision shape behind it (the
  neighbour check after placement reads them through `LevelRef::collision_boxes`).
- **Text alpha in `BlockEntityDrawContext::text`** (display boards' 0.85 / 0.975), and more than one colour per
  line in tooltips (goggles). The same call has no name tag mode (`Font.DisplayMode.SEE_THROUGH` with a
  background colour): a frogport's address over it on hover (`renderNameplateOnHover`) is drawn as its normal
  pass only, without the translucent box behind it or the faint copy seen through blocks.
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
- **Hanging entities in a box** (`Level.getEntitiesOfClass` over an `AABB`): `LevelRef::entities_in` leaves
  item frames and paintings out, so a schematic saved by the port (`StructureTemplate.fillEntityList`) holds
  none; one that holds them prints them. Listing them in `entities_in` (or a lookup of its own) would close it.
- **Calling a block's `setPlacedBy` from code with no player**: the schematicannon writes a door's, tall
  plant's and bed's other half itself as those blocks' `setPlacedBy` does (`glue/schematics`), where Create
  calls the block's own (a piston's extension check too). `PlayerRef::place_block` runs it, but needs a player
  in that level and writes with `NEIGHBORS`, where `placeSchematicBlock` writes `UPDATE_CLIENTS |
  UPDATE_KNOWN_SHAPE` with none; a `LevelRef` call (or a flags argument) would close it.
- **The local player's inventory slots on its client** (`player_inventory_slots` answers on the server only;
  `player_inventory` on a client lists stacks without their slots): the wand of symmetry's mirrors, drawn for
  every wand in the hotbar (`SymmetryHandler.onRenderWorld`), are sent to the client by the server.
- **A turned item in a screen** (`GuiGameElement.of(stack).rotate(..)`): the wand of symmetry's screen draws the
  wand flat.
- **Setting an entity's sprinting** (`Entity.setSprinting`, server side): the cardboard sword's blow doesn't stop
  the attacker's sprint (`CardboardSwordItem` replicating `Player.attack`); `is_sprinting` reads it only.
- **The creative tabs and their contents** (`CreativeModeTabs.tabs()`, `CreativeModeTab.contains`): the
  `in_item_group` filter attribute is refused; `build_creative_mode_tab_contents` lets a mod add to a tab but
  nothing reads one. A query of the category tabs and whether a stack is in one would close it.
- **Marking an entity immune to fire** (`Entity.fireImmune`, Create's `CreateFireImmune`): a player in a whole
  netherite diving set takes no fire damage (`living_incoming_damage`) and stops burning each tick, and their
  own view shows no flames, but lava still sets them alight for a tick, so others see flames on them.
- **Moving a playing sound** (a tickable `SoundInstance`'s `getX/Y/Z`, read each tick): turning machinery's
  ambient hum and cog clicks (`SoundScapes`, `ContinuousSound`) follow the mean of their blocks in Create; a
  `SoundRef` can change its volume (`fade`) but not its position, so they play at the listener with the volume
  their distance gives them, from no side (`glue/sources/ambience.rs`); a train's horn plays where the train was
  when it started (`glue/trains/horn.rs`). A `SoundRef::move_to(position)` would close it.
- **A particle drawn as its own quads** (`SteamJetParticle.render`'s four quads turned along the jet, or a
  `CustomRotationParticle`'s turn): steam engines' and whistles' steam jets are Create's sprites on a
  billboard, and so are the haunted bell's souls, where Create turns a soul only about the vertical and lays
  its base flat on the ground (`SoulParticle`, `SoulBaseParticle.getCustomRotation`). A rotation mode on
  `ModParticle` (a fixed quaternion, or a vertical-only billboard) would close both.
- **The level in a particle provider's tick** (`ParticleProvider::tick` has no host): a soul the light
  reaches is not taken away (`SoulParticle.tick`, `SoulBaseParticle.tick` reading `isDark`); the server
  spawns them only where it is dark.
- **The client's pick** (`GameRenderer.pick`'s hit result, replaceable by a client hook, as Create's
  `ExtendoGripItem.dontMissEntitiesWhenYouHaveHighReachDistance` sets `mc.hitResult`): the extendo grip lends
  the game's entity interaction range its reach, so an entity before a block within reach is picked, where
  Create picks entities that far only when the block pick missed. `client_attack_target` covers attacks
  only and can't let a block win.
- **A tinted particle with a block sprite and an exact velocity** (Create's `FluidStackParticle`: the fluid's
  still sprite, its tint times 0.8, the spawn's velocity, gravity and collision): the item particle
  (`ParticleOptions::item`) moves as Create's does but is drawn untinted, and the block particle takes a tint
  but its constructor randomises the velocity (`TerrainParticle`), so pipes pour and drip milk, tea, honey,
  chocolate and lava as Create does, and water and potions in their colour but scattered (`glue/fluids/flows.rs`).
  A tint on `ParticleOptions::item` (`rCol/gCol/bCol`) would close it.
- **A texture on world quads** (catnip's `Outliner` `withFaceTexture`, drawn with
  `RenderTypes.outlineTranslucent(texture)`): the super glue's areas are outlined as Create's are, but their faces
  are not covered with `AllSpecialTextures.GLUE` (`SuperGlueSelectionHandler`); `WorldDrawContext::quads` draws with
  the white atlas texel, and a mod pipeline's shader samples only a block or item atlas sprite
  (`RenderPipeline::textured`), where Create's special textures are files of their own (`textures/special/`).
  A texture for `quads`, or `PipelineTexture` of a mod's texture file, would close it.
- **A screen layout for a mod menu opened without a block** (`player_open_mod_menu` with no `pos`: the
  client's `open_native_screen` returns without a `Screen`): the filters' and the schedule's screens are the mod's own screen,
  and clicking an inventory slot carries a ghost copy, not the stack (Create's menus carry the real one), so the
  creative middle click on a ghost slot (a real full stack of it on the cursor, `GhostItemMenu.clicked` with
  `CLONE`) has no cursor to put it on. A `Screen` for a menu type (not only a block entity's), or a `screen`
  part hook keyed by menu, would close it.
- **An item's tooltip lines**: the filters' screens show a slot's name, not its tooltip. (An item's tags are
  found by testing the tags a data reload listener lists under `tags/item`.)
- **A block state's model drawn in the world without a block entity** (`SchematicRenderer`'s ghost blocks):
  a deployed schematic is outlined, not drawn; `draw_model_instances` takes only registered standalone models,
  not any block state's. The placement helpers' preview is the same want, translucent: catnip's
  `PlacementHelpers.tick` shows the block a click would place as a ghost (`GhostBlocks.showGhostState(..)
  .breathingAlpha()`), for the shaft and cog helpers, copycats, girders, table cloths, sails, ladders and
  display boards; a `WorldDrawContext` call drawing a `BlockState` at a position with an alpha, for one frame,
  would close both (the helpers themselves place as Create's do).
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
- **An additive blend for a block entity's drawing** (`RenderTypes.additive()`): a nixie tube on a train signal
  draws the signal's panel and lit tubes, but not the glow round each lit tube (`SIGNAL_RED_GLOW`,
  `SIGNAL_YELLOW_GLOW`, `SIGNAL_WHITE_GLOW` and the translucent `SIGNAL_WHITE_CUBE`, scaled to the light,
  within 128 blocks): `BlockEntityDrawContext::block` draws a model in its own layer only. A blend (or
  render-type) option on `block`/`terrain_model` would close it (`pipeline_quads` are world quads of a mod
  shader, outside the block entity's pose and with no block texture).

## Left out of the port, not for want of API

- Contraptions: contraption controls on a bearing's structure are not carried (their setting doesn't turn its
  actors off, `ContraptionControlsMovement.startMoving`), and a bearing's glue stays in the world while it turns,
  going back turned with it; an elevator's contraption controls choose no floor (`ElevatorControlsHandler`, the
  floor list and `ElevatorTargetFloorPacket`).

## Not to send

- The earlier port's `feat/create-hooks` branch on OlaHulleberg/horizon added `native_*` hook points for the
  mixin bundle. The fork's API replaced that route; none of it is needed.
