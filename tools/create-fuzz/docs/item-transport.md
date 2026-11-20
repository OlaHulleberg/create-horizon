# Player items and transport (version 3)

An `item` action has a typed `input`. Plain stacks have `item` and `count`;
`transport.json` lists supported items, stack limits and the dry depot block.
The Java server audits this contract against the running binary at startup.
Unknown items, oversized input stacks and unsupported transport-world machines
fail validation before either adapter runs.

```json
{"tick":40,"pos":[0,8,1],"action":{"kind":"item","input":{
  "kind":"drop_on","stack":{"item":"minecraft:iron_ingot","count":16}}}}
```

`drop_on` is a **controlled landing input**, not a simulation of vanilla flight.
Java calls the ordinary actor's `Player.drop`, positions the resulting item entity
at the segment's landing point with zero vertical motion, and invokes the block's
`updateEntityAfterFallOn`. Create's `BeltBlock.entityInside` then performs its
normal capability insertion. A depot invokes `SharedDepotBlockMethods.onLanded`.
No adapter inserts directly into a Java item handler or constructs a transported
stack. The unaccepted entity remainder is recorded and removed from the flight
world, so this action represents exactly one landing, not repeated retries.
Rust follows `logistics::item_landed` and `belt::put_on_from_above`, including the
transportable-slope check, stack limit, insertion-side occupancy, stored direction
bit and deferred inventory insertion. The player drop/flight/collision machinery
outside this boundary is not compared. Rejected drops remain visible in outcomes.

```json
{"tick":160,"pos":[0,8,5],"action":{"kind":"item","input":{
  "kind":"use","stack":null,"face":"up","yaw":0,"sneaking":false}}}
```

`use` calls `ServerPlayerGameMode.useItemOn` with the supplied stack (or empty
hand), face, yaw and sneak state. On a belt, empty main hand takes stacks within
0.55 of the segment centre; a nonempty ordinary ingot use does not place it on
the belt. On a depot, top-face use takes the held stack and puts down the offered
stack. Rust mirrors `belt::use_belt` and `depot::use_depot`; it calls `depot_put`
for entity-owned motion. Existing ordinary empty-hand `use` actions on these
blocks use the same adapter. A non-top depot click passes through.

Snapshots contain cumulative `item_outcomes`: position, offered stack, remainder
and stacks taken. Java measures actual actor inventory changes, treating items
returned to an initially empty selected hand as pickup, rather than an offered
remainder. Counts are aggregated by item id; slot layout is not compared.

`transport` appears on belt controllers and depots, including empty inventories.
Each item records its plain stack, longitudinal position, lock and pending state.
Active belt items also report whether the last tick left their position unchanged
(or the belt is stopped); pending items and depot items use null for that field.
Stack identity/count, presence, pending/lock/stall and positions are compared.
Items are sorted by position, id and count. Positions use the existing numeric
tolerance. Random render angles and their lateral jitter are not longitudinal
transport observations and are not synchronized.

Java observes Create's live queues using reflection confined to MIT classes:
BeltInventory.toInsert/toRemove and DepotBehaviour.heldItem. It does not call
BeltInventory.write, which flushes queues and would change the state being
observed. Belt debug NBT is therefore omitted; all semantic belt fields remain.
Rust reads live items and a cloned `to_save` view to identify pending items.
Repeated snapshots do not flush the engine's queues.

The logistics phase ticks depot sliding, then takes `busy_belts` once, ticks each
inventory using its actual solved speed and surroundings, and preserves items
added during transfers. It runs before the kinetic solve, as the glue does.
Surroundings implement ordinary stone/redstone solid endings and direct belt or
depot input. Belt insertion preserves from-belt offsets, side-entry rules,
occupancy, stopped/opposite-direction refusal and locks. Occupied depots refuse
insertion. Empty receiving belts follow the glue's busy snapshot, including any
observable delay; the driver does not tick extra empty belts to match Java.

The family uses player-connected belts with blocked ends, depots and a second
belt. It covers accepted/refused landings, multiple queued stacks, both motor
signs, reversal, source removal/replacement, stopped/blocked stalls, empty-hand
pickup, depot placement/swap/pickup, and per-tick histories through movement and
handoff. Mutation changes offered counts within the audited limits and retains
the existing interaction mutations. Manual cases isolate depot landings and
same-item swaps. Loaded-belt slicing, pulleys/casing, processing machines,
funnels/tunnels, fluids, item components/packages, depot output buffers and
native ejected-entity flight require further adapters. They are not claimed as
item transport coverage. Unsupported machine worlds/structural use actions are
rejected explicitly; generated ends do not eject into unmodelled native flight.

## Tunnels, covers, dyes and item access

Tunnels are placed by the player's tunnel item on a belt's top face. Java runs
`BeltTunnelItem`; Rust follows the glue: the default state is placed, and the logistics
phase's `tick_tunnels` cases the belt in the tunnel's own kind, gives the shape from the
outputs beside the segment (`hasValidOutput`) and from tunnels or extended belt funnels on
both ends (windows), and breaks a tunnel without a cased horizontal belt. The wrench toggles
a window (`onWrenched`, without neighbour updates); a sneak wrench takes the tunnel.
Andesite junctions hand single items to depots and belts beside the segment while more than
one is left (`BeltTunnelInteractionHandler`). Brass tunnels are generated only where they do
not distribute: a distributing brass tunnel is an adapter gap with a note.

Snapshots add `tunnel` (`BeltTunnelBlockEntity.flaps` keys and `sides`, by name),
`funnel` (`extractionCooldown`) and `chute` (its item and position) where those block
entities stand, and the `belt` observation adds `covered` and `color`. Rust computes a
tunnel's flaps from its current neighbours as the glue does.

## Funnels and chutes at belts

Belt funnels are placed by a state event on a belt: side funnels (pulling) and retracted or
extended funnels facing against the belt's way. The engine's `checkForFunnels` runs through
the driver's `funnel`/`funnel_insert` surroundings, which mirror the glue's `BeltSide`: an
andesite funnel puts the whole stack into the inventory behind it, a depot through its item
handler. A floor funnel at a belt's end takes the stacks that reach it only facing up and not
extracting (`supportsDirectBeltInput`), into the depot under it; any other funnel there holds
them at the end. Java keeps a stack a funnel emptied in the belt's list until the next tick;
both observers report such a stack as `minecraft:air` with count 0. A funnel-held item counts
as stalled from its second tick at the entry, as Java's `prevBeltPosition` comparison does;
the engine keeps that in `at_funnel`. Extracting funnels are adapter gaps with a note; brass
funnels' filters are covered below. `funnel` observes `extractionCooldown`, 0 in every mode the driver runs.

A vertical chute at a belt's end takes a stack while it is empty and no chute above leads
into it (`canDirectlyInsert`); with no fans it falls at gravity's `chute::MOTION` and leaves at
the bottom into a belt segment (from above), a depot, or the chute below, or out of the bottom
(native item flight), and waits at the middle over a solid block (`handleDownwardOutput`).
`chute` observes its stack and, while it holds one, the position. Diagonal and smart chutes and
fans are covered by the chute columns below; chutes taking from an inventory above are outside the
adapter.

Dyes and water buckets colour or wash the whole belt (`applyColor`); an unchanged colour
passes the click to the item. The connector carries the clicked segment's colour to a
segment it adds and to a belt it joins. A cased horizontal segment is covered while a stone,
redstone block, casing or depot stands on it (`updateCoverProperty` on a change above, or on
casing); a tunnel and a funnel not facing up never cover it, and taking the casing off keeps
the flag, as in Java. Item handler access (`capability`, see player actions) reaches depots
and belt segments.

## Brass tunnel distribution

Brass tunnels over parallel belts form a group across their axis (`isConnected`: both have
flaps). One that distributes (`hasDistributionBehaviour`: flaps, and connected or a flap across
its axis) takes a whole stack off its belt or from a belt ending into its segment while it
holds none (`setStackToDistribute`), and from its item handler (`BrassTunnelItemHandler`).
The driver mirrors the glue's `tick_brass`: two ticks after a stack arrives (then every
`brassTunnelTimer`) it gathers the group's outputs (`addValidOutputsOf`: its own sides across
the belt first, then ahead; depots and belts beside it, the belt ahead) and splits the stack
among them as the selection mode says, filtered sides first (`tunnel::distribute`);
synchronizing tunnels hand out only while the whole group holds stacks. The mode is picked
with `scroll_setting`, side filters with a click on their slot (plain items only). A brass
tunnel's saved state follows the glue's column record (`brass_to_nbt`: stack, mode and the
side it came in by).

`tunnel` adds `brass` for a brass tunnel: the stack it holds and the side it came in by,
`distributionProgress` (only while it holds a stack or hands out with its group: `tick` reads
it only then, and a tunnel that never held one starts at 0 while an emptied one is -1),
`previousOutputIndex`, `syncedOutputActive`, `connectedLeft`/`Right`, the mode and the sided
filters. `RANDOMIZE` starts at `level.random` on Java and is never generated; a stack thrown
off the belt ahead (native item flight) is an adapter gap with a note, and the family keeps a
belt segment ahead of every tunnel. `DistanceLeft`/`DistanceRight` are written and read by
`BrassTunnelBlockEntity` and read nowhere else in Create, so they are not an observable.

A stack a funnel or tunnel emptied is reported as `minecraft:air` with count 0 on both sides
(the port's `count == 0` item is an empty stack).

## Weighted ejectors

An ejector (its default target, one block ahead: `EntityLauncher(1, 0)`) is driven by a motor
on its shaft. The driver mirrors the glue's `tick_ejectors` after logistics (the arms
subsystem): its redstone (`tick_arm_signals`), the engine's `Ejector::tick` with the glue's
`EjectorWorld` (its depot's stacks, the target's simulated insertion, the obstacle scan through
`clip_blocks` over any block but air, extracting funnels let through), `land` into what stands
at the target (an ejector merges, a depot takes while empty, a belt from above), stacks landing
on it and belts ending into it merging onto what it holds (`depot_insert`), its item handler
(`DepotItems`), and its stack size (`scroll_setting`). Ejectors wake when their depot changes
(`wake_ejectors`). A stack the throw drops (native item flight) and a belt funnel on top are
adapter gaps with a note; entities on the plate are not generated.

`ejector` observes its state, power, plate (`lidProgress`), launcher distances, stack size,
the stacks in flight (ticks flown, stack) and `earlyTargetTime`; the plate and the time compare
within the transport tolerance. The depot observation covers ejectors too and lists an ejector's
merging stacks (`DepotBehaviour.incoming`) as pending items.

## Funnels pushing from a depot

Belt funnels move the stacks of the depot behind them (its `DepotItems`): one facing with its
belt (retracted or extended: the shapes a player can give a funnel along a belt) or a pushing
one across it puts one item at a time onto the belt from its side while the belt takes items
there (`activateExtractingBeltFunnel`); one on a depot pushes onto it while it is empty (a stack
unless it is a pushing funnel) or, pulling, takes its resting stack into the depot behind
(`handleBeltFunnelOutput`). The driver mirrors the glue's `tick_funnels`: each funnel is due
again the next tick, `EXTRACTION_COOLDOWN` + 1 ticks after a stack left, and never while dormant;
`funnel` observes `extractionCooldown` as the ticks before its next turn. Redstone pauses them.
Extracting funnels (their stacks fly off as item entities), inventories other than depots and a
funnel over an ejector are adapter gaps with a note; brass funnels' filters and amounts are below.

Depot observations add the comparator output (`getComparatorInputOverride`): the held stack and
outputs over the held stack's size (an ejector's stack size), as the glue's `analog.rs` reads it.

## Depot output buffers

A depot's output buffer (`processingOutputBuffer`) is driven by a spout over the depot (the fluid
items family's world: a creative tank, pump and pipe feeding a spout two blocks over a depot)
filling a stack of glass bottles (honey) or sweet berries (chocolate): every filled item but the
last goes into the buffer. The driver keeps the buffer's eight slots in place (the glue's block
entity slots `depot::OUTPUTS`), so the item handler's slots 1..8 (`DepotItemHandler`) reach the
stack the buffer put there, simulated or not; inserts are refused while any slot holds a stack, as
are landings (`item_landed`); a player's empty hand takes everything; and a pulling belt funnel on
the depot exports the outputs, then the resting held stack, into the depot behind it
(`handleBeltFunnelOutput`). Funnels pushing from a depot take from its held slot and outputs
together (`extract_matching` over the slots). Merging is an ejector's alone (`enableMerging` is
called only by `EjectorBlockEntity`); a mismatched merging stack drops (native item flight), and
oversized stacks cannot enter through the plain-stack contract.

## Brass funnels' filters and amounts

Brass funnels at belts (taking off the belt, pushing onto it from the depot behind, on a depot
beside the belt pulling or pushing, and facing up at the belt's end) get a filter by a click on
the filter slot (`FunnelFilterSlotPositioning`) while their amount board does not show
(`acceptsValueSettings` false: `ValueSettingsInputHandler` calls `onShortInteract` on the
server). While it shows, the server does nothing on the click: the client sends a
`ValueSettingsPacket`, the `filter_setting` action, either the release of a short click (its
`interact` item and face: `onShortInteract`) or a board pick (row 0 "up to", row 1 exactly, and the
count: `setValueSettings`). The driver mirrors the glue's `use_funnel_filter`, `value_settings`,
`funnel_passes` and `funnel_extraction`. `funnel` adds a brass funnel's filter, its count within
the filter's stack size (a count above it moves no more) and whether it is "up to". Extracting
funnels stay outside the comparison: every stack they move flies off as an item entity, and the
next extraction waits for that entity to leave (`lastObserved`), which is native physics.

## Chute columns

Columns of chutes at a belt's end: vertical and diagonal chutes in the shapes `updateChuteState`
settles them in, a smart chute with a filter (a click on a side slot) and redstone beside it, and
an encased fan under the column blowing its stacks up into a depot over the top. The driver
mirrors the glue's `tick_chute_shapes`, `tick_chutes` (falling and rising, `chute_motion` from the
fans' push and pull, the diagonal chute's target), `chute_accepts` and the belt end's
`direct_input_accepts`. The chutes are placed before the belt, so that Java ticks them before it as
the port's logistics phase does (that order is F5). Chutes taking from an inventory above or lifting
entities and belt items from below (`handleInputFromAbove`, `findEntities`, `extractFromBelt`),
encased chutes and stacks blown out of the top (native item flight) are adapter gaps with a note.
