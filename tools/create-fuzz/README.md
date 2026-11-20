# create-fuzz

Differential fuzzer for this port's engine (`create/src/engine`). It runs the same random setups in
[Create](https://github.com/Creators-of-Create/Create)'s Java (on NeoForge 1.21.1) and in the engine, compares what
they settle to, and shrinks every difference to a minimal case.

It is a development tool: nothing here is built into or shipped with the mod.

- `java/`: a small NeoForge test mod. One headless dedicated server runs scenarios in isolated, serial namespaces.
- `rust/`: the generator, the engine driver, the comparison and the shrinker.
- `cases/`: hand-written scenarios.

Kinetics first: motors, cranks, shafts, cogs, gearboxes, clutches, gearshifts, gauges and consumers.

## Setup

- A Create checkout of the `mc1.21.1/dev` branch next to this repository (`../Create` from its root), or named
  by `CREATE_DIR`. Build it with its own Gradle on JDK 21 and publish it locally: `./gradlew publishToMavenLocal`.
- JDK 21 and Rust 1.99.1.

The driver builds the engine from this checkout. Run the commands below from `tools/create-fuzz`.

## Run

```sh
./run.sh server                          # start the Java side; waits until it is ready
./run.sh fuzz fuzz 0 1000                # seeds 0-999; differences go to found/<seed>.json
./run.sh fuzz shrink found/123.json      # the smallest case that still differs: found/123.min.json
./run.sh fuzz check cases/*.json         # hand-written cases
./run.sh stop
```

To drive another checkout's engine (a worktree), set `CREATE_HORIZON=/path/to/checkout`: the driver is built
against it in its own folder (`FUZZ_BUILD`, default `work/builds`), and several drivers can share one
server. `FUZZ_FOUND` sets where `fuzz` saves differences.

### Engines

The driver re-implements the port's glue (`rust/src/engine*`) and mirrors this repository's `main`:
its engine API and its glue's order and rules. A driver built against an older checkout may not
compile; one built against a branch that changes the glue differs where the branch does.

The server queues requests by arrival (file modification time, filename only to break ties).
After each case is disposed and its cleanup has settled, an unfinished batch goes to the back
of the queue. Small checks and fluid-host RPCs therefore wait for case quanta rather than
whole batches. A running case is never interrupted; its duration still bounds responsiveness.
Suspended batches retain only inputs and results; namespace resets, leak assertions and
response order within each batch are unchanged.

The scheduler regression test needs only JDK 21:

```sh
javac -d /tmp/fair-inbox-test java/src/main/java/dev/createfuzz/RequestQueue.java java/src/test/java/dev/createfuzz/RequestQueueTest.java
java -ea -cp /tmp/fair-inbox-test dev.createfuzz.RequestQueueTest
```

The server writes `work/default-states.json` when it starts: every Create block's default state from the running
binary (property and value names as serialized). `tools/create-data` takes it as
`tools/create-data/src/default_states.json`.

`./run.sh server` accepts Minecraft's EULA for its local test server (`java/run/eula.txt`).

Differences are filed as issues with the shrunk case, not kept here.

## Scenarios

```json
{"seed": 0, "ticks": 40,
 "blocks": [{"pos": [0, 1, 0], "state": "create:creative_motor[facing=east]", "motor": 32}],
 "events": [{"tick": 20, "pos": [1, 1, 0], "state": "minecraft:redstone_block"},
            {"tick": 25, "pos": [0, 2, 0], "turn": false}]}
```

Initial state specifications are injected one per tick, in order. `motor` sets a creative motor's speed the tick after it is placed.
`day_time` (default 0) is the level's day time when the case starts; Java sets it for each batch, whose cases share
it. Each event happens before its tick runs: `state` places a block (`minecraft:air` removes it), `turn` turns a hand
crank (true: backwards). A `minecraft:redstone_block` powers the blocks beside it.

## How the two sides are kept comparable

- Initial states are injected one per tick: Create's outcome depends on which block entity ticks first.
  Player item use is a distinct version 3 action through the interaction chain.
- A motor's speed is set the tick after it is placed, as a player scrolls it; set at placement, Create starts it
  before it attaches.
- Java's tick t is the level tick after the one it is placed in; each case is read after its own last tick.
- Every case area is force-loaded when the server starts. At a sprint tick rate, block entities placed in a chunk
  that is still loading wait 10-20 ticks before they tick.
- The day time is the scenario's: Create's mysterious cuckoo clock explodes at 18:32, so a server whose day ran on
  between batches blew up some cases and not their reruns. Clock animations and the explosion are machine work
  (group 7): the Rust driver does not run them yet, so generated cases keep day time 0, where none happens.
- Clearing a slot also discards its entities: a cleared depot or belt drops its items, which landed on the next case's
  depot in that slot and made it java-unstable.
- A difference is rerun three times on Java first; a case Java disagrees with itself on is reported as unstable.
- `rust/src/allow.rs` lists the differences confirmed not to be port bugs, each with its evidence.
- A placed block's state starts from the port's `default_state` (`create/data/create/sunrise/block/<name>.json` in the
  checkout `run.sh` drives, `CREATE_HORIZON_DIR`), as the game's placement starts from the block's default.
- The port's glue rules the engine lacks (a crank breaks when its support goes) are mirrored in `rust/src/engine.rs`.

## What is not here

No Minecraft, NeoForge or Create jars, no Create assets, and no world saves. Gradle downloads the dependencies at
build time; `java/` is MIT code that calls their public APIs. Never commit their jars, the built harness or anything
from their sources. The licence is this repository's MIT licence.

## Coverage guidance

```sh
CREATE_DIR=/path/to/Create FUZZ_PORT=25611 ./run.sh server
./run.sh fuzz cover 300
./run.sh stop
```

The server uses pinned JaCoCo 0.8.14 (downloaded into `work/jacoco`, never committed).
JaCoCo and ASM jars are checked against pinned Maven Central SHA-1 values on every use, including
cached jars; mismatches are deleted and stop the command.
`FUZZ_COVERAGE_INCLUDES` is a colon-separated JaCoCo include list; it defaults to
`com.simibubi.create.content.kinetics.*:com.simibubi.create.api.stress.*:com.simibubi.create.api.registry.*`,
including the kinetic network, torque propagator, stress values and their registry helpers. Fluid, belt, depot, funnel, tunnel, chute and package classes are included by the default wrapper; the belts report counts chutes with them. `FUZZ_CREATE_JAR` selects the matching published Create binary.
Each guided case resets the agent after area cleanup and dumps probes before publishing its snapshot.
Offline analysis unions those probes before counting branches, so discoveries on opposite edges of the
same line count separately. The agent dumps only included Create classes into `work/coverage-classes`: analysis uses these
pre-instrumentation binaries where runtime transformers changed Create, and the published jar for
classes never loaded. Class ID mismatches fail analysis.

`corpus/` (override `FUZZ_CORPUS`) keeps full scenarios that increase cumulative branch coverage. It is
ignored because it is generated, binary-version-dependent run data. Existing scenarios are replayed on
each run before new rounds; 80% of new rounds mutate a corpus member, the rest explore fresh seeds.
Mutations add/remove/replace blocks, adjust motor speeds or event ticks, keeping legal generated states,
bounded positions and a settling window. Differences and instability use the same recording path as
`fuzz` (`FUZZ_FOUND`). `work/coverage/classes.tsv` lists every included class, `never-reached.tsv` lists
class/method/line and the number of missed branch edges there, `summary.txt` totals the edges, and
`seeds.tsv` records each case's new edges. JaCoCo does not assign source-level names to individual
edges: the missed list groups edges by method and line. The union `.exec` and retained scenarios allow
further analysis; `cases/` in the report keeps every scenario and its isolated `.exec` probes. Coverage includes background execution in the measured interval; it describes the
whole server's Create kinetics, including classes unsupported by the current scenario grammar.

## Coverage contract and observations

`./run.sh fuzz manifest cases/*.json` reports all 73 kinetic ids, Cartesian state-domain
sizes, distinct generated specifications (seeds 0–1999), manual specifications and supported
interactions. Specifications may omit properties and use Java defaults; these counts are not
exhaustive state coverage. `rust/src/kinetics.json` is the explicit MIT source-derived contract.
The Java adapter audits every id/property/value against the loaded Create binary at startup,
including detecting additional kinetic registrations. The audit corrected the research table:
`rope_pulley` has only `axis`; waterlogging belongs to its nested rope/magnet blocks.
Unknown ids, properties, values and external inputs fail before either adapter runs.
Direct state placement is manual solver coverage, not player interaction coverage. Motor settings, hand-crank turns and the version 3 inputs documented below have
interaction adapters. The manifest does not claim
execution of any omitted subsystem or player-reachable state closure.

Scenarios additionally accept `checkpoints: [0, 1, 2, ...]`, measured after that many completed
ticks and before the next actions. The extension is backward compatible. Checkpoints are opt-in:
transient propagation may depend on Java block-entity iteration order; the usual repeated Java
runs still classify instability. A checkpoint at `ticks` duplicates the final semantic observation.
At tick t both adapters first place initial block t, then set initial block t−1's motor, then
apply all delayed event motor settings in array order, then execute tick-t events in array order,
then run the level/engine tick. Final observations occur before any further actions. In particular,
a same-tick place followed by air removes the block. Ambiguous events and out-of-range checkpoint
schedules are rejected. Shrinking retains checkpoint history.

Snapshots include **every position**, including air, in the closed bounding box of all initial
and event positions expanded by one block on each side. The box must fit the isolated Java slot
(x/z −24..24, y 0..24). This observes neighbour-created blocks and surviving blocks outside the
submitted positions. Empty scenarios observe an empty region. Gauge comparator output is read
from the actual Java gauge and Rust gauge state; it is compared along with source presence and
sequence angle/distance effective limits. Rust part reconstruction uses saved motor settings and
actual steam output for dynamic capacity, rather than an unconditional default settings object.
Java continues to call the actual impact/capacity methods. Steam sensing/programming itself is
outside this group's action grammar and is not covered by these observations.

Both adapters record network membership as sorted position partitions and per-position
network sizes. Rust reads the selected part's actual membership through
`Kinetics::network_membership`; split shafts select the same driven half used for kinetic
figures. Raw network ids are used only to group positions within each snapshot and are never
serialized or compared. An empty network observation is emitted as empty partitions and sizes.
Dynamic settings other than the currently driven motor/crank inputs and readable steam output,
environment sensing, belt/conveyor links, placement outside the version 3 subset and machine
work remain outside differential coverage until their action adapters and required accessors exist.

The shrinker tries contiguous block/event chunks before individual removals so long checkpoint
histories remain practical to minimize. It still finishes with the individual candidates; reducing
case duration never drops a requested checkpoint. Submission counts in `manifest` distinguish
crank-turn and motor-setting actions from state specifications.

Static geometry now comes from each manifest entry's typed `connection` rules (`geometry.rs`):
axis selection, shaft face predicate and small/large cog classification. The Java startup audit
checks these rules against every generated block state's actual `IRotate` and `ICogWheel`
methods. Adding a block requires its manifest contract, rather than a generator kind list.

The static kinetics families (see [seed families](#seed-families)) enumerate the catalogue's
domains, test weighted crank stress boundaries and grow networks through compatible shaft faces.
Geometry cases probe all six adjacent faces
(including nonconnections), output shafts, planar and diagonal gears and perpendicular large
gears. Repeated domain passes reverse insertion order. Motors reverse after a source removal;
a settled checkpoint preserves the initial drive observation. The stress boundary cases test loads
4, 8 or 12 per source rpm against crank capacity 8, with a 2:1 cog speed ratio. These are the
adjacent load steps in that fixture. Static specifications include waterlogging. Contained fluid fixtures use the native fluid scheduler through the private host peer, described below. Static geometry remains state injection; dedicated fluid histories use player placement.

`manifest` includes an explicit static exclusion reason for each omitted id: belt/conveyor
links (group 5), environmental generators (group 6), moving/assembling machines (group 7),
and independent static chain-shape injection. Transmission traces exercise chain
placement/topology, controller placement/boards, sequencer programs, valve boards/use/dye
and crank expiry. Inherited crank turns are never used as valve activation. Machine work, crafting, crushing entities and equipment filling remain outside static solver tests. Dedicated fluid histories exercise transfer, filters, buckets and item machines.

Observation scope is explicit per block in the manifest's `observation_scope` map:
only its named properties are omitted by the shared comparison; raw snapshots retain
them. Chain part and connection coordinate are committed and fully compared.
Display up/down and girder top/bottom belong to shape updates, gantry part/carriage axis to
their updateShape methods, and large-wheel extension to LargeWaterWheelBlock.updateShape.
Placement-selected facing/axis/shaft flags remain compared: state injection supplies
those inputs, rather than invoking player placement. Fluid valve enabled state and kinetic observations are compared through the fluids phase; pointer records are persisted and tested across reload.

Kinetic generation remains dry. Contained fluid-world cases can use waterlogged states:
the Rust adapter reads and changes the native environment through the private server's
separate host-fluid region. See [the fluid world boundary](docs/fluid-world.md) for callback
semantics, isolation, observations and the server requirement for `fuzz rust` fluid cases.
Network sizes read the existing TorquePropagator registry's KineticNetwork.getSize(),
without constructing networks; partitions still group block entities by network id.

Version 3 routes typed item uses through the player interaction chain, including
clicked positions, face hit points, orientation and sneaking. It also exposes block
breaks and controlled analog input for a bounded subset of blocks. See [player actions](docs/player-actions.md) for
schema, exact entry points, the ordinary ServerPlayer with packet-discarding
connection, the Create FakePlayer audit and explicit remaining exclusions.

Transmission screen settings are version 3 `speed_setting`, `valve_setting` and
`program` actions. Java uses the ordinary actor and the value behaviour or
ConfigureSequencedGearshiftPacket.handle; Rust calls the corresponding world
APIs and runs their input ticks. See docs/player-actions.md for payloads and
accessor limitations. Per-tick traces observe sequencer runs, valve use/angle and
context; controller targets and chain modifiers are also compared.

`shrink <case.json> [witness]` can retain a particular difference substring while
reducing a case with several findings. Every accepted candidate still has only
original differences, and at least one original difference containing the witness
must survive. Omitting the witness retains the original shrink policy. Coverage
reports can reside on a different filesystem from the server's work directory.

The belt family now drives persistent belt
connector clicks, horizontal/sloped/vertical/sideways chains, both click orders,
refusals and casing changes followed by segment removal. Manual belt cases add
pulley insertion/removal and endpoint shortening/extension. Conveyor loop traces
use `chain_link`, the server transaction sent after the player's chain selection.
Snapshots compare controller/index/length/casing and sorted conveyor link offsets.
See [item transport](docs/item-transport.md) for player stack landings/uses, movement, belt/depot handoff, stalls, per-tick observations and explicit native-flight/component boundaries. Inventory-backed splitting remains separate.

The belt slicer family cuts and joins loaded belts:
interior wrench splits on both sides of a segment (creative, and refused in survival, whose
actor holds no connector or shafts), shortening from either end, lengthening, joins with and
without a flipped partner, the maximum length, and the connector's refusals (cross axes, range,
obstacles, cross shafts, opposing driven ends, a forgotten first shaft) and its 5-tick item
cooldown, which the driver models as the game does for the port.

The tunnel family places andesite and brass tunnels on
belts beside depots and cross belts, with neighbouring tunnels, the wrench's window toggle,
outputs taken away and back, stacks through andesite junctions, and the tunnel losing its
casing or belt. The belt surface family dyes, washes, cases, covers and
pulleys horizontal, sloped, vertical and sideways belts, then breaks a segment. The depot
family inserts into and extracts from a depot through its item handler
(`capability`), with belt handoffs, player pickups and a belt segment's own handler.

The funnel family puts belt funnels over loaded belts with
depots behind them, a floor funnel at the belt's end, and player pickups that free a blocked
funnel. The chute family ends belts at vertical chutes over depots, belts,
chutes, stone or nothing, and under a chute leading into them.

Four more cover depot output buffers (a spout fills a stack on a depot, in a quarter of the
seeds inside an encased fan's current past lava, `engine/fans.rs`; item handlers,
pickups, landings and a pulling belt funnel reach the buffer), brass funnels' filters and
amounts (with the `filter_setting` action), chute columns (diagonal and smart chutes and
encased fans at a belt's end) and presses over depots and belts. Each family's residues are in
[Seed families](#seed-families). See [item transport](docs/item-transport.md).

The water wheel family drives a wheel's flow from a
contained falling-water column, both flow signs, a lava column, the plank material through
player uses (a non-block item, the wheel's own spruce, an invalid block and a valid plank),
player-placed wheels along and across the existing wheel's axis, and the large wheel's room
check, item retry at the clicked face, extension train, controller cells, frame material,
sneak wrench and break cascades. The Rust driver asks the native fluid host for
`FluidState.getFlow` at each wheel's sample cells and runs the frame's scheduled shape
reconstruction, so flow scores and controller cells are compared per checkpoint.

The steam engine family builds tanks with active
(blaze burner fed a blaze cake) and passive (magma, a lit or unlit campfire) heat, water
supplied by a creative actor's bucket, floor/wall/ceiling engines, a waterlogged boxed-in
engine, valid and same-axis shafts, the engine's and powered shaft's placement helpers, a
sneak wrench, engine removal and a tank block removal. The driver counts engines per
boiler, reads the heat under the tank, samples the water supply, converts shafts to powered
shafts and back through `PoweredShaftBlock.getEquivalent`, and ticks blaze burners.

Fluid histories use `fluids.json` state domains and version-3 player uses for pipe/pump/tank placement, wrench/glass/copper casing, creative actor bucket exchange (`game_mode`), smart filter slots, and redstone-controlled valve transmissions. `fluid_inputs.rs` drives creative tanks, drains, depot/belt spouts and a startup-audited component-free recipe subset. Observations include tank shapes/contents, six connection pressures and flights, world fluid state, machine storage/cycles/held items, filters and depot output slots. Rust reads the engine's stored per-side pressure, flow progress and open-end buffers directly, including sources retained by a live network. Component-bearing potions, arbitrary recipes, contraptions and item entity flight are outside the admitted contract.

Shrinking evaluates one candidate at a time. Check/fuzz batches are bounded by observed checkpoint cells and JSON records stream through buffered readers/writers. On the shared VM, run runtime sweeps and shrinks one at a time under `prlimit --as=3000000000`, after checking at least 4 GiB is available.

Found records are written as compact `.json.gz` files (with fluid lanes a raw record can exceed 100 MB); set `FUZZ_RECORD_GZIP=0` for plain JSON.
The default remains plain JSON for existing coverage tools. `rust`, `java`, `check`,
`shrink`, `repeat-java`, `compare-records` and `diff-files` accept gzip inputs by suffix.
Compression preserves both complete snapshots and every difference for triage.

Kinetic network histories (`rust/src/network_inputs.rs`) drive the network and transmission blocks the way a player builds them: cogs, gauges,
gearboxes, chain drives, controllers, sequencers, cranks and valve handles placed with `use`
against competing shaft neighbours, clicked faces, sneaking and look directions; casings,
girders, brackets and wrench turns or pickups on relays, encased cogs and chain drives in and out
of a chain; competing and opposed sources joined, retargeted and removed while running; cranks
turned holding items; valve rims clicked through their cooldown; sequencers powered in the same
tick. The driver mirrors the port's glue for these inputs (chain wrench turns, value boxes, the
crank's use, unbracketable targets). Each history's doc comment names its Create inputs;
the kinetics gap report (`./run.sh fuzz gaps kinetics`) lists what remains.

## Seed families

`rust/src/generate.rs` deals each seed to one scenario family by its residue modulo 128
(`FAMILIES`). Every residue has exactly one owner: the table is checked when it compiles, and a
unit test lists the residues that changed owner. Families written for consecutive seeds (water
wheels, steam engines, source faces, catalogue states) pick their variants by the seed's index
among their own seeds, so every variant is reached. A seed with `seed & 32` set also reloads
(`persistence.rs`); a family with one residue mod 128 either always or never reloads, as the
table's comments say.

| Family | Residues | Share | Why |
| --- | --- | --- | --- |
| 20 kinetic network histories | one each mod 64: 1, 4-7, 20-22, 24, 25, 32, 33, 36-38, 52-54, 56, 57 | 20 × 1/64 = 31% | Newest player-built histories; their variants use the seed's bits from 64 up, so each keeps a whole residue mod 64 |
| fluid_pipes, fluid_world, fluid_pumps, fluid_inputs, fluids, transport | 16, 17, 18, 19, 8, 9 mod 32 | 6 × 1/32 = 19% | Long histories with variants keyed on the seed from bit 32 up; unchanged |
| belts | 15 mod 16 | 1/16 | Unchanged: belt findings cite these seeds |
| belt funnel_push, chute_columns, brass_funnels, depot_outputs | 23, 35, 39, 55 mod 128 | 4/128 | Belt logistics histories of the third round; each keeps the half of its `belts-cover` lane (23, 35, 39, 55 mod 64) whose seeds the issues cite. funnel_push never reloads, the other three always do, as on `belts-cover` |
| belt press_processing, brass, ejectors | 78, 110, 126 mod 128 | 3/128 | Presses never reloaded on `belts-cover` (7 mod 64); brass tunnels and ejectors always did (38, 32 mod 64): kept so |
| belt slicer, tunnels, funnels | 99, 103, 61 mod 128 | 3/128 | Reload; the slicer picks its cut, join or refusal variant by `index` |
| belt surface, depots, chutes | 94, 29, 66 mod 128 | 3/128 | Never reload |
| transmission valve, sequence, chain, generate | 10-12, 14 mod 16, below 64 mod 128 | 4 × 1/32 = 12.5% | Short traces with few variants (bits 16 and 32, seed / 64 mod 3); they gave up the half above 64 |
| transmission crank | 13, 45 mod 128 | 1/64 | Its trace reads no seed bit but the reload one: one residue of each half |
| transmission flicker | 2 mod 128 | 1/128 | Reads no seed bit and 2 mod 64 never reloads: 66 generated the same trace |
| waterwheel | 74, 90, 106, 122 mod 128 | 1/32 | Back after the network campaign took its residues; its pre-campaign share |
| steam_engine | 75, 91, 107, 123 mod 128 | 1/32 | As water wheels |
| actions | 3 mod 64; 87, 119 mod 128 | 4/128 = 3% | A few variants of uses on one shaft, clutch or gearshift: 3 mod 64 has `seed % 8 == 3` and `% 32 == 3`, 87 and 119 the `% 8 == 7` half |
| geometry/states | 77, 93, 109, 125 mod 128 | 4/128 = 3% | Static kinetics: one catalogue state per seed by index, every block, face and order; each state is still reached, over more seeds |
| geometry/stress_boundary | 0 mod 64 | 1/64 | Unchanged: every eighth static kinetics seed |
| geometry/network | 92, 124 mod 128 | 1/64 | Random networks through compatible faces (unreached since the fluid families) |
| geometry/source_faces | 76, 108 mod 128 | 1/64 | Six input faces of gearboxes, clutches and gearshifts (unreached since fluid_pumps took 18 mod 32) |
| geometry/cog_boundary | 34 mod 64 | 1/64 | Unchanged |

Residues that changed owner (mod 128): transmission valve's 74, 90, 106, 122 to waterwheel;
sequence's 75, 91, 107, 123 to steam_engine; chain's 76, 108 to source_faces and 92, 124 to
geometry/network; crank's 77, 93, 109, 125, generate's 78, 94, 110, 126 and actions' 87, 99,
103, 119 to geometry/states.

The belt logistics families (from `belts-cover`, whose lanes the network campaign had taken)
then took: actions' 23, 35, 39, 55 (funnel_push, chute_columns, brass_funnels, depot_outputs);
geometry/states' 78 (press_processing), 94 (belt surface), 99 (slicer), 103 (tunnels),
110 (brass), 126 (ejectors); transmission/crank's 29 (depots) and 61 (funnels);
transmission/flicker's 66 (chutes); geometry/states' 87 and 119 went to actions. A belt issue
that cites a `belts-cover` sweep seed reproduces from its `.min.json`; the seed itself generates
the same history only at 23, 35, 39 or 55 mod 128.
