# API gaps

What the mod SDK (Sunrise's mod API, also proposed for Horizon) can't do yet, at the build pinned in
`create/horizon-mod.toml`. The mod leaves these behaviours out (or approximates them, as stated) and the
module doc in `create/src/glue/` says so too. Re-check an entry against a newer SDK before acting on it
(`cargo horizon-mod sdk diff <old> <new>`).

Each entry: what Create does → what's missing → the API that would close it.

## World and blocks

- **Chunk save moment.** Column data is written into its chunk attachment at the end of the tick that changed it
  (`glue/persist.rs`) → no hook before a chunk saves → a hook at the moment before a chunk is saved.
- **Shape from a block entity.** A factory gauge's outline follows its panels (`FactoryPanelBlock.getShape`); here it
  is always the whole face → shapes are per state only → a block-part shape hook (outline and collision, with position).
- **Scoreboard.** Scoreboard and death-counter display sources are left out → no typed scoreboard access → host calls
  to list an objective's scores and to add a dummy objective / set a score.
- **`isSolid` of a declared shape.** A declared block keeps its template's `isSolid`, so wall signs and torches drop off
  some blocks (packagers, frogports and postboxes use solid templates to avoid it) → `calculateSolid` isn't run on
  declared shapes → compute it from the collision shape, or a `solid` declaration field.
- **Light emission and friction of a state.** Copycats can't glow or slide as their material → nothing reads another
  state's light or friction → `HostBlockStates::light_emission(state)` and `friction(state)`.
- **Other block-state facts.** `isRedstoneConductor`, `isSolidRender` (approximated by "full collision cube"),
  `requiresCorrectToolForDrops` (deployers dig everything at harvest speed), the weathering-copper class and age, a
  block's class (copycats guess from tags and ids) → `HostBlockStates` facts for each.
- **A state's numeric id.** Break effects of wrenched copycats, casings, brackets and the belt slicer are rebuilt from
  particles and the break sound, not the client's own burst → `BlockStates::state_id` (or `level_event(2001)` taking a
  `BlockState`).
- **Neighbour-shape update of a state placed by code.** The wand of symmetry's mirrored placements skip
  `updateShape` → a placement call that runs it.
- **Level in `OnBlockPlacedByEvent`.** Decor fix-ups after a placement with no placer are skipped → a level on the event.
- **Scaffolding collision for a templated block.** Metal scaffolding can be stood on but not climbed through → the
  game gives scaffolding's context shape only to `minecraft:scaffolding` → a declaration field for it.
- **Fluid interactions in declarations.** Honey or chocolate meeting lava turns it a tick later, from `fluid_tick` →
  no declaration for `registerFluidInteractions` → a content-declaration entry.
- **Redstone in `state_for_placement`.** A clutch or gearshift placed beside power takes `powered` a tick later →
  `LevelHost` has no `has_neighbor_signal`.
- **A placement answering another block.** A shaft placed in front of a steam engine becomes a powered shaft a tick
  later (`pickCorrectShaftType`) → `state_for_placement` can't read another block's state.
- **Collision shapes in `can_survive`.** Hand cranks and valve handles are refused only against air or fluid →
  `LevelHost` has no collision shapes.
- **`setPlacedBy` from code without a player.** The schematicannon writes doors', tall plants' and beds' other halves
  itself → `PlayerRef::place_block` needs an online player → a `LevelRef` placement that runs `setPlacedBy`.
- **Hanging entities in a box.** Schematics saved by the mod hold no item frames or paintings → `LevelRef::entities_in`
  leaves them out.
- **Portal destination.** No portal tracks or `train_portal` → nothing finds or builds the exit portal →
  `LevelRef::portal_destination(portal, axis)` (the railway would then need cross-level graphs).

## Containers, items and transactions

- **Rolling back mod state with a transaction.** Tanks offer no `FLUIDS`, belts and tunnels no `ITEMS`, upward saws
  no inventory, and portable storage interfaces only serve Create's chutes and funnels → a handler can't learn its
  transaction aborted → an abort callback on `Transaction`, or a journal for mod state.
- **Notice of any container changing.** Chutes under vanilla containers and frogports above inventories re-check on a
  timer → `contents_changed` reaches only the block's own parts.
- **Empty stack from `menu_quick_move`.** The schematicannon can't move a whole stack into its gunpowder slot → the
  hook can't return "empty" → `Option<ItemStack>`.
- **Per-slot insert rules on `ModContainer`.** "Insert only while empty" (depot, ejector, crafter, chute) and the
  toolbox's one kind per compartment for hoppers and funnels.
- **Block entity in `BlockDropsEvent`.** A toolbox destroyed by the level (`/setblock .. destroy`) drops empty.
- **Removing config files.** `ConfigFiles::remove` (oldest schematic upload) and access to the game's `schematics/`
  folder.
- **Slots over 99.** A toolbox compartment holds 256; only the deposit button and hoppers fill past one stack.
- **Creative tabs.** The `in_item_group` filter attribute is refused → nothing reads a tab's contents.
- **An item's tooltip lines.** Filter screens show a slot's name, not its tooltip.
- **Translations on the server.** Display links use Create's en_us text.

## Entities and moving structures

- **Moving colliders.** Players can't stand on or be pushed by bearing and piston structures, trains or minecart
  contraptions (trains still hurt and throw what they hit) → `LevelRef::set_moving_colliders`.
- **Mod entity types that aren't template copies.** Seats are tagged item displays; riding seats on moving structures,
  clicking their blocks and Create's package entity are left out; `EntityRef::vehicle` is server-only and
  `entities_in` skips displays, so the shop overlay guesses who is seated.
- **Display entities loaded from a save.** Not reported to `entity_join_level` nor listed by `entities_in`, so trains'
  displays are cleaned up with a `kill` selector on a per-opening tag.
- **`EntityRef::move_by`.** Belts carry mobs by setting motion and players by a client push (with step-up on slopes),
  and coupled carts only have their motion damped → no extra move for an entity.
- **Collision shape for a riding entity.** Every entity collides with a belt's cut shape → shapes select by entity
  class only.
- **`MinecartRef::set_carrying`.** A cart with a contraption doesn't move the ridden cart's step.
  (`MinecartRef::is_on_rails` also reads false in a level tick; the mod reads the rail.)
- **Mod projectile type.** Potato cannon shots are snowballs flown by the mod, hurting as `minecraft:thrown`, and lost
  as plain snowballs over a save → a projectile class of its own, or persistent data on `EntityRef`.
- **Damage types.** Create's damage types aren't registered, so crushing wheels use `cramming`, saws, drills, rollers
  and run-over trains `generic`, fans `lava` and `in_fire` → register a native mod's `damage_type` files.
- **Enchantments.** `/enchant` applies `create:capacity`; tables and anvils offering Capacity and Potato Recovery is
  unverified.
- **Fake player attack strength.** A deployer's punch deals about a fifth of Create's damage → a
  `PlayerActionSetup` field for full charge.
- **`Block.fallOn` distance.** Seats halve a fall by recomputing the damage → a writable `distance` on `FallOnEvent`.
- **Sprinting.** The cardboard sword doesn't stop the attacker's sprint → no `Entity.setSprinting`.
- **Fire immunity.** A full netherite diving set still catches fire from lava for a tick → no `fireImmune` flag.
- **Fluids and swapped sprites on display entities.** On trains and cart contraptions, tank, basin, spout and drain
  fluids, dyed or cased belts and wall banners aren't drawn right → a fluid display, or an entity renderer with
  `ClientHost` and `BlockEntityDrawContext`.

## Client and rendering

- **Client light.** Moving structures and steam jets use the server's light, and a filter in a smart chute is drawn
  full bright → `LevelRef::brightness` is server-only.
- **Local player state.** Air (diving helmet bubbles still drop), ground flag and swimming (diving boots), inventory
  slots (symmetry mirrors come from the server), flight (`player_abilities` on the client reads the server's last
  abilities packet, so the server tells the fans' client push whether its player flies) → client-side reads or
  writes for each.
- **Worn stack data in place.** A backtank's air update replays the equip sound → `ItemStack` change without
  `setItemSlot`.
- **The local player's swing.** The extendo grip's arms move only on a raw mouse click.
- **The client's pick.** The extendo grip picks entities within its reach before blocks; Create lets blocks win →
  a hook replacing `GameRenderer.pick`'s result.
- **Consuming a key in `input_key`** (train driving).
- **Whether an item model is 3D.** Belts, depots, placards and link slots approximate `isGui3d`.
- **Keeping only part of a block entity's drawing.** A block with one animated part redraws every frame; the clockwork
  bearing can't hide its top for the hour hand.
- **Colour over a block-entity model.** The overstress red/green flash is left out → `terrain_model` tint only reaches
  tinted quads.
- **Text in block-entity draws.** No alpha (display boards) and no see-through name-tag mode (frogport address).
- **Sound for everyone but one player.** The cardboard sword's bonk also plays from the server to the clicker.
- **Moving a playing sound.** Machine hum and cog clicks play at the listener; a train horn stays where it started →
  `SoundRef::move_to(position)`.
- **Changing a playing sound's pitch.** An air current's wind keeps the pitch it started with; Create's follows the
  player's speed each tick (`AirCurrentSound.setPitch`) → `SoundRef::set_pitch(pitch)`.
- **Particle rotation and level access.** Haunted-bell souls are billboards (Create turns them about the vertical)
  and don't vanish in light → a rotation mode on `ModParticle` and a host in `ParticleProvider::tick`.
- **Tinted fluid particles.** Pipes spray water and potions scattered, not at Create's velocity → a tint on
  `ParticleOptions::item`.
- **Connected textures.** Joins decided by block entity data or collision (vault and tank seams, crafters, brass
  tunnel tops, copper roofs, casings next to encased blocks) are left out → a per-face sprite shift reading
  neighbour states and block entity data.
- **Menus without a block.** Filter and schedule screens are the mod's own and carry ghost copies → a `Screen` for a
  menu type.
- **A block state drawn without a block entity.** Schematic ghost blocks are outlined only → a `WorldDrawContext` call
  drawing a `BlockState` with alpha.
- **Translucent block models.** Placement-helper ghosts are drawn from quads `create-data prepare` bakes out of the
  jar's models, one textured pipeline per sprite (`glue/interaction/ghost.rs`) → an overall alpha (a translucent
  route) for `draw_model_instances`, so the mod draws the block model itself and drops the bake.
- **Items and text in world draws.** Filter slot items and counts (`FilteringRenderer`) are left out.
- **A turned item in a screen.** The wand of symmetry's screen draws it flat.

## Game bugs found (Sunrise `b77574eb1b`)

- **One bad recipe empties its type.** `recipes_of_type` fails the whole list when one result names an unknown item
  (Create 6.0.10's `chain_from_zinc` names `minecraft:chain`). The mod renames the chain in its recipes. Fix: skip
  recipes that fail conversion.
- **`_head` blocks get a skull block entity.** `create:mechanical_piston_head` gets `minecraft:skull` because
  `BlockEntity::for_block` matches any `_head`/`_skull`. Fix: check the `minecraft:` namespace.

## Not done, API not needed

- Contraption controls on a bearing's structure don't turn its actors off; a bearing's glue stays in the world while
  it turns; elevator contraption controls choose no floor.
