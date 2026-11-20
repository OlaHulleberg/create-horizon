# Fluid world boundary

Fluid scenarios use the existing version-3 `use` primitive. Buckets first enter block use;
when it passes, the Java adapter positions the player's eye on the selected hit-face normal
and calls the ordinary game-mode item-use path, including the bucket's real ray cast. Rust
applies the corresponding host input. Direct bucket exchange with Create handlers also
needs the actor's game mode, handled separately from world bucket use.

`fluid_world.rs` defines the external block catalogue and normalized fluid-state read:
fluid id, source flag, amount and falling flag. Those ids occupy a separate range from
Create's engine state table. `engine/fluid_world.rs` implements the port's
`glue/fluids/open_end::Front` callbacks: source drain/renewal, placement, waterlogging,
cauldron/beehive drains and spill reactions. Open ends retain their own partial bucket.

Native fluid scheduling belongs to the host. `fluid_host.rs` and Java `FluidHost` run that
host boundary in a separate region of the same pinned Java binary used for comparison.
Each Rust case owns a suspended namespace. Its blocks and pending native fluid ticks are
materialized only during a synchronous request; only the public native fluid scheduler
is advanced explicitly. The shared server keeps ticking between requests. Java Create block-entity ticks, scheduled block ticks, kinetic
solves and fluid-network transfers do not run in this region. Rust owns those Create
subsystems. Java's normal placement/shape callbacks may run when a host state changes;
only native block changes and waterlogging are returned to Rust. Other Create state
properties, tank contents, pressures and flows are never imported.

Each frame applies Rust's ordered host writes at the preceding completed tick, advances
the native fluid scheduler once, then applies the glue's explicit due fluid ticks. The
Create fluid phase and kinetic solve follow on Rust. This provides the same native
1.21.1 environment on both sides without reproducing or consulting Minecraft's or
NeoForge's implementation sources. It measures port behavior under that controlled host;
it does not establish equivalence of every native behavior in the port's target game.

The service keeps independent namespaces for concurrent runners and releases each when
its Rust run ends (or its owning process exits). Ordinary reference batches run serially
through the same server while these namespaces remain suspended.
`fuzz rust` therefore also needs the server for fluid-world scenarios. Pure engine and
callback unit tests remain independent of the service. Ordinary dry scenarios keep the
standalone driver path.

Fixtures must contain their fluid within their observed region and isolated server slot.
The host rejects escaping fluid explicitly. The admitted catalogue covers water, lava,
honey, chocolate, cauldrons, beehives and the supported Create waterloggable blocks.
The current server is the overworld. Source-conversion rules and water evaporation are
read from the live host and supplied to the Rust callbacks. Nether
water evaporation, arbitrary third-party fluids/handlers, entity effects, fire/candle
workloads and custom dimension rules need separate environment contracts.

Fluid cases observe the input bounding box expanded by two blocks, so a pipe placed beside
a click and the world cell beyond its open end are both included. Java's normal runner and
the Rust host peer enforce the same containment boundary and clear rejected slots before
fluid can reach other cases. Rejected inputs are recorded as `driver-unsupported` and do
not enter the coverage corpus. Shrinking rejects these candidates through the same
comparison contract.

The native peer also returns replacement, collision and face-support queries. Open-end
admission uses those host facts, including partial vanilla slabs. Waterlogging and source
callbacks use the actual fluid-state reads at the last native frame, with local writes
visible immediately until the next frame.

Connection observations normalize all six sides and account for active, retained and
network-held open-end buffers, deduplicating handler identity locally. Raw identities are
never compared. Both adapters observe stored connection pressure and flow state.
Rust's `pressure_model=stored_connections` reads the engine's per-pipe transport
behaviours directly, including open-end source objects shared with network caches.
Pressure, flow and transfer comparisons remain enabled.


Machine observations preserve cycle counters, storage and held-item state. The existing belt
`stalled` observation uses native previous-position equality on Java and the engine's stored
queue/end stalled flag on Rust; machine-held stillness can differ by representation. Those
comparisons remain enabled and are reported as an observation-model limit, separately from
cycle, contents, lock and position differences. The driver does not infer a port bug from that
flag alone.

World bucket input uses the same native peer. Vanilla block use (including cauldrons and
secondary-use bypass) and the bucket's real item raycast run synchronously without advancing
a tick. Create block-use handlers run exclusively in Rust; the peer skips them. It returns
only environmental states/facts and waterlogging, never Java tank/filter/flow contents. This
also handles an occluding source above the originally clicked block when an empty bucket
raycasts it. The ordinary and sneaking cauldron fixture checks both paths.

Snapshots retain the native batch `origin` as adapter context. Before a use callback, Rust
reconstructs `(origin + block + hit) - (origin + block)` so its glue receives the host's actual
double-precision hit. This matters for equal-distance glass-wrench choices at decimal hits:
the same local decimal can round differently in different batch slots. Comparisons remain
strict; there is no geometric tie allowance. Dry standalone Rust traces use origin zero.

The native peer participates in `CaseIsolation`: each frame enters a fresh namespace,
restores its blocks, block-entity data and exact scheduled fluid ticks (including priority
and tie order), and saves them before clearing the entire materialized region. Each peer
has its own actor and deterministic random source; the ambient random source and game time
are restored before returning. No peer blocks, entities, scheduled ticks or Create networks
remain live between requests. Existing empty-level, containment and build-stamp assertions
continue to apply. Native block ticks and block events remain unexecuted, as in the frozen
host contract. `FUZZ_DIR=<private server directory> python3 tools/test-fluid-host.py` checks
interleaved peer frames, ordinary batches and disposal after a rejected third peer. Fluid boundaries and escaped entities fail explicitly. Unobserved item
drops are counted and disposed while still inside the owned region; escape checks run first.
Normal reference cases also assert containment before disposal. No boundary failure is
converted into a partial successful case.

Failed reference requests dispose their batch before checking emptiness or publishing an
error. Rejected peers also clear the containment guard ring, including its scheduled block
and fluid ticks. A secondary cleanup error is reported with that request and does not
escape the server tick listener. The namespace regression exercises rejection and malformed
input between live peer frames and verifies that the same server boot remains usable.
