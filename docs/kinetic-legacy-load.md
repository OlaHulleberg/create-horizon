# Loading saves without kinetic state

This is a migration of an older create-sunrise save format, **not Create behaviour**.
Create's persistence and rotation propagation remain in their own implementation paths.
The migration is implemented in `create/src/engine/src/kinetics/legacy_load.rs`.

Older builds saved drawing fields (`Speeds` and `Theory`) and motor settings, but no
`KineticState`. The kinetic block entity load callback detects that format after column
settings have loaded. A newly placed block entity has neither old drawing data nor a
snapshot, so it follows the normal placement lifecycle. A block with `KineticState`
follows the normal restore lifecycle. The client update tag still excludes server snapshots.

Before first attachment, the engine groups connected legacy-loaded parts and any saved
parts they touch. For an entirely legacy component, candidate generators are tried in
lexicographic `(x, y, z, port)` order. Each trial infers a source tree using the engine's
connection and conveyed-speed rules. Ratios around loops must agree, rotation must stay
within the speed limit, and every other generator must be a valid member at the inferred
speed under the creative motor initialization rule: `generated <= theoretical`, or
`abs(theoretical) >= abs(generated)` with matching signs. The first valid candidate wins.
A component with no generator or turning saved anchor is restored as stationary.

Mixed-format components preserve the saved states. Their saved network id identifies the
source generator; legacy parts fill the missing tree around it. Registration into a saved
network uses silent enrollment, subtracting the legacy part's contribution from the saved
unloaded totals before adding its live contribution. This also applies when a part first
loaded in a separate legacy component and joins the saved network on a later tick.

Startup inputs that change a pending legacy part update its configuration before inference,
without starting fresh-placement propagation.

Successful inference is installed as restored state, without calling destructive rotation
propagation or replaying player placements. Loaded legacy parts retain migration provenance
for this session, so later column arrivals can reconcile earlier partial components using
the same stable order. Saved-only worlds do not enqueue migration walks until a legacy part
has actually been encountered. Ordinary player placements never enter this inference path.

A mixed component whose saved source column is still absent is deferred. It is not given a
cyclic invented source tree and its old block entity data is not replaced with an empty
snapshot. Later loaded parts retry the component. Likewise, topology with no consistent
candidate is deferred instead of destroying blocks on import. The migration only promises
reconstruction of a consistent old network; it cannot recover historical placement order
or repair an inherently conflicting network from data the old format never recorded.

After successful inference, the existing change-driven block entity writer saves normal
`KineticState`. Subsequent reloads use Create's persistence path rather than this migration.
The migration queues and provenance are transient; no new persistent side store is added.
