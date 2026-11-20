# Version 3 player inputs

Version 1 state-injection cases remain readable. The direct-method version 2
format is rejected: its action positions were placement targets and cannot be
interpreted as clicked blocks without migration. The checked-in cases now use
version 3. Exactly one of `state`, `turn` and `action` is required per event.
Events retain array order and checkpoints observe completed ticks.

```json
{"version":3,"day_time":0,"ticks":80,
 "blocks":[{"pos":[0,7,0],"state":"minecraft:stone"}],
 "events":[
  {"tick":2,"pos":[0,7,0],"action":{"kind":"use","item":"create:shaft","face":"up","hit":[0.5,1,0.5],"yaw":90,"pitch":0,"sneaking":false}},
  {"tick":4,"pos":[0,8,0],"action":{"kind":"use","item":"create:wrench","face":"up","hit":[0.1,1,0.5],"yaw":90,"pitch":0,"sneaking":false}},
  {"tick":6,"pos":[0,9,0],"action":{"kind":"signal","strength":7}},
  {"tick":8,"pos":[0,8,0],"action":{"kind":"remove"}}
 ]}
```

`pos` is the clicked block for `use`. `hit` is a point relative to that block's
lower corner. `face` is the hit surface normal; a partial block's surface can lie
inside its containing unit cell. The adapter bounds the point to that cell but
does not perform a host ray cast. Yaw/pitch are degrees. The Java adapter
sets an ordinary ServerPlayer's body/head rotation, sneak state and held item, and calls
**ServerPlayerGameMode.useItemOn**. This runs RightClickBlock, the block's item
interaction, the held item's use, and normal BlockItem placement. There are no
direct adapter calls to Create's wrench/casing/bracket methods, placement
callbacks or overridden clicked positions. A sneaking wrench use removes casing
or picks up an ordinary block through that same chain. `remove` remains a
separate game-mode block break. The player holds 64 items for each independent
use; inventory, drops, sounds, reach validation and client screens are not observed.

Rust mirrors the supported glue dispatch in order:

1. Interaction's RightClickBlock value-box handler. Motor geometry uses the same
   `board::motor_slot` and `board::hits_box` engine functions as the glue.
2. Non-sneaking block use-item-on handlers: bracket removal before the wrench;
   shaft encasing and shaft placement assistance (`place::pole_helper`).
3. Item use-on: bracket placement (`decor::bracket_for_click`), wrench subsystem
   overrides then default rotation (`wrench::rotated`), or block placement
   (`place::axis/facing`, neighbour shaft queries and committed world callbacks).

Wrench rotations and casing swaps call **CreateWorld::switching_block_state**
before committing, exactly as `items/wrench.rs::switch_state` does. The
`actions-wrench-network.json` regression verifies member counts 3 → 2 following
end-shaft rotation. The earlier 1511/#14 finding was a missing driver call, and
has no allowance. Network partitions and counts remain compared in full.

The supported item set includes shaft, creative motor, clutch, gearshift, gearbox,
flywheel, chain drives, adjustable chain gearshifts, controllers, wrench, both shaft
casings, both bracket items, empty hand and vanilla dyes. Shaft bracket state
is separately compared. Casing items that are not consumed by an encasing handler fall through to normal
block placement, including when sneaking. Unsupported targeted block overrides
produce explicit notes, keeping the case and comparison. Other value
boxes, cog/pipe brackets, cog casing/toggles, fluid placement, world collision,
lever/wire/conductor simulation and arbitrary machine block overrides
remain outside this driver's supported transactions; the manifest names these
limits. The standalone binary cannot instantiate the SDK host glue itself.

The Java actor is exactly `ServerPlayer`, not a subclass and not a `FakePlayer`.
Each case owns its actor for its batch. `PlayerActor` attaches an ordinary
`ServerGamePacketListenerImpl` to a `Connection` subclass that overrides all three
public `send` overloads: outgoing packets are discarded and optional send
callbacks complete successfully. No socket, inbound packet processing, login,
player-list/world registration or client screen is simulated. Server-side item
use and block break remain the normal player game-mode calls. The runtime
`actor-probe.json` asserts the actor's exact class and verifies that a normal
player message reaches the packet sink. `RuntimeProbe` uses the same actor
factory for the angle measurements.

The motor's centre and off-centre regressions both use ordinary-player hit tests:
a centre hit is consumed before the wrench, and an off-centre hit reaches the
wrench. The old fake-player gap notes have been removed, without removing those
cases or relaxing their comparisons.

### Public API construction

Only published API signatures and binary experiments were used, without reading
Minecraft or NeoForge implementation sources. The documented constructors are
[ServerPlayer](https://mappings.dev/1.21.1/net/minecraft/server/level/ServerPlayer.html),
[ServerGamePacketListenerImpl](https://mappings.dev/1.21.1/net/minecraft/server/network/ServerGamePacketListenerImpl.html)
and [Connection(PacketFlow)](https://mappings.dev/1.21.1/net/minecraft/network/Connection.html).
The listener uses [CommonListenerCookie.createInitial](https://mappings.dev/1.21.1/net/minecraft/server/network/CommonListenerCookie.html)
and the public `ServerPlayer.connection` field. The sink overrides the three
public `Connection.send` overloads and completes the documented
[PacketSendListener.onSuccess](https://mappings.dev/1.21.1/net/minecraft/network/PacketSendListener.html)
callback. There is no reflection, bytecode inspection, access transformer,
private-field access or copied host implementation in this construction.

### Create FakePlayer audit

The audit searches Create's MIT `src/main/java` for `FakePlayer`, including
`DeployerFakePlayer` and constructions/imports, then follows the generated
use/break/placement paths. Paths below are relative to that source directory.

| Reachable check | How this adapter relates to it |
|---|---|
| `foundation/blockEntity/behaviour/ValueSettingsInputHandler.java:75`, `onBlockActivated` | Every eligible generated use reaches the event handler. `instanceof FakePlayer` is false, so `testHit` is required and the fake-player short-interact branch is not selected. This fixes the off-centre motor artifact. |
| `foundation/blockEntity/behaviour/scrollValue/ScrollValueBehaviour.java:183`, `onShortInteract` | The method's fake-player fallback to the block's `useItemOn` is false for our actor. The motor's ordinary value-settings branch does not call that fake-player fallback; any ordinary short interaction remains a no-op here. |
| `foundation/advancement/AdvancementBehaviour.java:127`, `setPlacedBy` | Kinetic block placement calls this helper. The fake-player early return is false and the ordinary ServerPlayer branch is eligible. Its separate world-player lookup cannot resolve an unregistered headless actor, so ownership/advancement delivery is not established by this suite and is not compared. |
| `foundation/mixin/BlockItemMixin.java:21`, `create$fixDeployerPlacement` | Applies at the normal BlockItem placement entry point. Our actor is not a DeployerFakePlayer, so its deployer-only rejection of clicks on non-replaceable blocks is not selected. |
| `content/kinetics/deployer/DeployerFakePlayer.java:110`, `deployerHasEyesOnHisFeet` | The globally registered dimensions-event handler can run during actor construction/dimension refresh. Its subtype check is false; ordinary player eye height is retained. |

The complete search also finds the following guarded or out-of-scope checks;
none is silently treated as ordinary-player interaction coverage:

- `content/redstone/link/LinkHandler.java:52` is in a global RightClickBlock
  subscriber, but returns before its fake-player slot-selection check when the
  target has no LinkBehaviour (all current generated use targets). The actor
  would take geometric, rather than fake-player slot selection if this subsystem
  were added.
- `content/kinetics/chainConveyor/ChainConveyorConnectionHandler.java:77` is
  client-only and requires a conveyor target and chain item; these uses are not
  generated and no client is simulated.
- Target-specific checks in `RedstoneRequesterBlockEntity.java:150`,
  `PackagePortBlockEntity.java:140`, `PackagerBlock.java:81`,
  `ItemHatchBlock.java:98`, `BlazeBurnerBlock.java:164`,
  `ToolboxBlock.java:95,154`, `ClipboardBlock.java:131`,
  `BlueprintEntity.java:345`, `BacktankBlock.java:152`,
  `SeatBlock.java:133` and `SeatEntity.java:120` require targets/entities outside
  generated player uses. Static kinetic-state injection is not a player click
  on those targets.
- `BeehiveBlockMixin.java:17`, `MinecartContraptionItem.java:223` and
  `DeployerHandler.java:189` concern bee anger, mounted contraption items and
  deployer/villager trading; no such actions/entities are generated.
- `DeployerFakePlayer.java:118,140,146` checks drop/XP ownership and retaliation
  targets; block use/break cases do not perform living-entity attacks.
  `DeployerMovingInteraction.java:47` and
  `DeployerMovementBehaviour.java:277` test a moving contraption's stored
  temporary actor, not our player; moving contraption interactions are excluded.
  Autonomous Create deployers may construct their own fake players during static
  machine ticks; those do not replace the scenario actor.
- `PloughBlock.PloughFakePlayer` and the potato cannon's zombie-converter actor
  are constructions/subclasses rather than additional generated-player checks.


Signals use `createfuzz:signal[strength=0..15]`, whose real Java getSignal and
getDirectSignal supply the requested strength to Create's neighbour queries. Rust
uses the strongest adjacent source and passes its exact strength to
set_chain_signal. Redstone blocks supply 15. Strength sweeps remain unchanged.
Placement beside power retains the documented API_GAPS note for unavailable
placement-time redstone; no persistent kinetic difference is suppressed.

At the measured exact yaw 45°, pitch 0° tie, Java 1.21.1 has equal absolute
sin/cos bits (1060439283), whereas the port's cosine is 1060438146 and selects
WEST instead of SOUTH. `trig.rs` gates on this exact input and placement API
provenance: getNearestLookingDirection must select the axis/facing, with no
preferred-neighbour or clicked-face rule deciding it. Horizontal-direction APIs
are excluded. Adjacent floating-point angles do not qualify.

The ordinary engine trace always supplies the compared kinetic, network, item,
bracket and other observations. For qualifying placements a separate state trace
uses the measured Java choice to derive the expected orientation, subsequent
wrench/casing state and the connected chain component's part/axis_along_first.
Only these proven properties can differ in an allowed state pair; all other
properties remain exact. Unrelated chain components are excluded. Raw port states
remain visible, with an exact-tie tag and the allowed property names in diagnostic
metadata. `RuntimeProbe.java` records the binary measurements at startup. Tests
retain non-tie, preferred-neighbour, unrelated-state, speed, capacity and network
membership-size differences. The comparison allowance is not keyed to a block id.

### Kinetic speed board submissions

Version 3 additionally accepts `{"kind":"speed_setting","row":0,"value":128}`
at a motor or rotation speed controller. Row 0 is negative, row 1 positive;
columns are 0..256. Column zero selects magnitude one, as Create's screen does.
Java calls the selected `KineticScrollValueBehaviour.setValueSettings` with the
ordinary case actor, the same behaviour entry used by the value-settings packet.
Rust calls `CreateWorld::board_motor`. Legacy `motor` placement settings now
use this path too. `speed_setting` is compared for both block types, including
unpowered controllers. Mutation changes both board coordinates.

`minecraft:air` is accepted as an empty hand use; the Rust adapter dispatches it
to `turn_crank` on hand cranks. `transmission-crank-expiry.json` checks refresh
before expiry, refresh at expiry, release, and reversal with per-tick checkpoints.

Chain drives and adjustable chain gearshifts now accept ordinary `use` placement.
The Rust adapter applies both properties returned by `take_chain_shapes` through
committed world updates, in glue order. Chain part and connection-coordinate
outputs are compared. Chain traces contain two adjustable gearshifts, an analog
0..15 sweep and boundary transitions, middle removal/replacement and both motor
signs. `chain_modifier` reads Java's getModifier; Rust reports the modifier of the
last signal submitted to set_chain_signal (the engine has no public setting getter).

`program` carries `instructions`, each with `instruction` (TURN_ANGLE,
TURN_DISTANCE, DELAY, AWAIT, END), `modifier` (FORWARD_FAST, FORWARD, BACK,
BACK_FAST) and `value`. Lists contain one to five screen rows; END is terminal.
Java constructs ConfigureSequencedGearshiftPacket and calls handle with the
ordinary actor within range. Rust calls set_sequencer_instructions(stop=true),
then checks redstone, ticks instructions and commits returned state transitions
in the same order as the glue. Snapshots compare current index, duration,
progress, timer and previous power; the block state remains compared too.
Sequence angle/distance limits continue to compare propagated context at every
kinetic block. Per-tick program traces include pulses during turns and await,
configuration while running, restart, both turn kinds and both modifier signs.

Valve handles accept `valve_setting {row,value}` (two sign rows, columns 0..180)
through angleInput.setValueSettings on Java. Rust stores the same entity-owned
scroll selection using board::kinetic_value; empty-hand use calls use_valve.
Dyes run through the ordinary `use` pipeline; Rust uses dye::recoloured and
commits the corresponding state while retaining the angle selection. The handle
snapshot compares selected angle, in-use ticks and getIndependentAngle(0).
Rust maintains the glue's accepted-use data (target, duration, direction) and
uses valve::drawn_angle; the world does not expose its private crank countdown.
Generated traces exercise busy use, angle thresholds, both directions/sneaking
and all sixteen dye items, with per-tick observations and propagated context.

Controller traces now place the controller with `use` below an existing large cog.
The adapter passes large_cog_axis to place::axis's Nearby context. Both legal
controller axes (x/z), opposite cog axes and both input routes are generated.
The y axis is absent from Create's controller property domain. The cog itself is
an initial fixture; large-cog item placement and its assistance are not claimed.

Chain trace variants also force x/y/z shaft axes, swap adjustable gearshifts
between line ends and middle positions, and reconnect a removed middle drive.
Their topology properties are compared without exclusions. Sequencer context
normalization reads the engine's outgoing half at the driven half's speed to
match Java's single block entity. A present zero context remains distinct from
absence, so stale contexts after expiry/end are findings.

The driver distinguishes committed state changes with neighbour notifications
(`UPDATE_ALL`) from the glue's client-only powered-property updates
(`UPDATE_CLIENTS`). Both call block_changed; only the former call neighbour
callbacks. Shape and sequencer-state commits continue to notify neighbours.

The port's `values::right_click` tests a valve box geometrically even while the
valve is busy. The Rust adapter follows that handler; it does not import Java's
inUse-dependent `isActive` condition. Thus an unsneaking centred dye click can be
consumed by the port before recolouring. Off-centre and sneaking clicks still
reach the block's dye/use handler. This source-level port/Java difference is
retained by comparison.

The earlier unit-boundary-only validation excluded real partial-block hit points.
Up-facing valve traces now hit their half-height gameplay surface, from Create's
MIT AllShapes definition. A centred surface hit (0.5,0.5,0.5) reaches the value
box, while an off-centre rim hit reaches activation/dye. This is a backward
compatible widening of accepted version 3 hit coordinates, not an asset mesh.

Belt connectors now retain their first-shaft component across `use` events on
one case's actor. Other use stacks retain the earlier independent-use semantics.
The actor's cooldown manager advances once per scenario tick. Use at least five
ticks between connector clicks. Rust invokes belt_connector::click/layout and
commits the same dry segment states, then settles controllers before solving.
Belt snapshots compare controller coordinates, index, length and casing kind.
The driver uses belt::break_cascade through BreakWorld, matching the glue's synchronous cascade. Existing belt interaction changes use switching_block_state; flipped and newly extended segments retain direct commits.

`chain_link {target:[x,y,z],connect:true|false}` submits the player's
ChainConveyorConnectionPacket on Java (with 64 chains in the held slot), and
mirrors packages::chain_command's Chains connect/disconnect and sync_far on Rust.
The target is a scenario-relative block position; provision both endpoints in
initial blocks. This exercises the server transaction after client selection;
it does not test the client's distance/slope/connection-count validation or
crosshair geometry. Only valid client-selectable links belong in this family.
Sorted relative link offsets are compared independently of network membership.

Item inputs, movement, belt/belt and belt/depot handoff, stalls and stack/position
observations are documented in [item transport](item-transport.md). Native flight,
loaded slicing and full interaction closure remain separate coverage boundaries.
No comparison field or allowance was removed.

`game_mode` with a boolean `creative` selects the real actor game mode on Java and the corresponding glue permission branch on Rust. Normal tank bucket exchange requires creative; creative tanks accept fluid selection from either mode. Fluid pipe/pump/smart pipe/valve and machine placement uses the same `use` primitive and custom placement callbacks. Smart filters use their actual slot hit region. Drain landings use the existing typed `item/drop_on` primitive; item flight before landing is outside this boundary.

### Item handler access

`capability` is the access another block makes (a hopper, a pipe, another mod): the
clicked block's item handler on `side` (null: the unsided one), then one
`insertItem(slot, insert, simulate)` or one `extractItem(slot, extract, simulate)`.
It is not a player input: the actor's inventory is untouched.

```json
{"tick":30,"pos":[0,8,0],"action":{"kind":"capability","side":"up","slot":0,
  "insert":{"item":"minecraft:iron_ingot","count":20},"extract":null,"simulate":false}}
```

Java calls `level.getCapability(Capabilities.ItemHandler.BLOCK, pos, side)`. Rust follows
the glue's `logistics/handlers.rs::provide`: a depot's `DepotItems` takes into the held slot
only while every slot is empty (up to the stack limit, the held stack arriving at position 0
as `depot_contents_changed` sets it) and gives from the held slot; belts and andesite tunnels
have no handler in the port. The outcome joins `item_outcomes` with `handler` (whether one
was given), the offered stack, the remainder and what was extracted. Other blocks' handlers
are adapter gaps with a note.

### Value board picks on scroll values

`scroll_setting` is a pick on a block's value board for its `ScrollValueBehaviour`, as the
client's `ValueSettingsPacket` sends it: `row` and `value`.

```json
{"tick":40,"pos":[0,9,2],"action":{"kind":"scroll_setting","row":0,"value":2}}
```

Java calls `setValueSettings(player, new ValueSettings(row, value), false)` on the block
entity's `ScrollValueBehaviour` (a `ScrollOptionBehaviour` is one: a brass tunnel's selection
mode takes the option's ordinal; a weighted ejector's `maxStackSize` takes the count, 0 for
any). Rust follows the glue's `interaction/values.rs`: a brass tunnel takes
`board::option_value` against its current mode and hands it to its group (`set_brass_mode`);
an ejector's stack size is `value` clamped to 0..64. The value board's opening hold and
`isActive` gate are the client's and are not part of this input.

A brass tunnel's side filter is set by the ordinary `use` primitive: the held item clicked on
the side's filter slot (`BrassTunnelFilterSlot`, the side's face at the slot's point), which
`ValueSettingsInputHandler` routes to `FilteringBehaviour.onShortInteract` on Java and the
glue's `use_tunnel_filter` on Rust. An empty-hand `use` off the mode slot on top takes the
stacks of the tunnel's group (`BrassTunnelBlock.useWithoutItem`); a plain `use` records no
item outcome.

### Filter boards

`filter_setting` is the client's `ValueSettingsPacket` for a block's `FilteringBehaviour`, which
the client sends only while the filter's amount board shows (`acceptsValueSettings`: a brass
funnel that supports an amount with a filter that stacks). With `interact` it is the release of a
short click on the filter slot holding `interact.item` (on `interact.face`); without it a board
pick: `row` 0 for "up to", 1 for exactly, `value` the count.

```json
{"tick":32,"pos":[0,9,2],"action":{"kind":"filter_setting","row":0,"value":0,
  "interact":{"item":"minecraft:gold_ingot","face":"up"}}}
{"tick":34,"pos":[0,9,2],"action":{"kind":"filter_setting","row":1,"value":16}}
```

Java does what `ValueSettingsPacket.applySettings` does for the filtering behaviour: nothing
unless `acceptsValueSettings`, then `onShortInteract` with the item in the main hand, or
`setValueSettings`. Rust follows the glue's `amount::value_settings` for brass funnels: the release
sets the filter (`set_funnel_filter`, a wrench or an arm doing nothing), the pick the amount
(`FilterAmount::from_settings`, the count within the filter's stack size). While the board does not
show, an ordinary `use` on the filter slot sets the filter on both sides (`onShortInteract` from
`ValueSettingsInputHandler`, the glue's `use_funnel_filter`); while it shows, that `use` does
nothing on the server. A smart chute's filter slot takes the same `use`.
