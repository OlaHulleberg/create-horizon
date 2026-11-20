# Kinetics observation boundary

The manifest retains the binary's full state domains. Generation uses only dry
states; both adapters reject waterlogged=true rather than deleting fluid results.
ProperWaterloggedBlock.updateWater, AbstractShaftBlock.updateShape,
HandCrankBlock.updateShape, DrillBlock.updateShape, PumpBlock.updateShape,
FluidValveBlock.updateShape, BacktankBlock.updateShape and
GirderEncasedShaftBlock.updateShape schedule vanilla water ticks. No fluid-world
adapter is present. This suite cannot establish water sensing/transfer parity.

The per-id observation_scope map contains property names and deciding MIT Java
methods. It applies symmetrically at comparison time, including checkpoints;
raw observations retain all values. It never removes kinetic fields.

| Blocks | Excluded output | Deciding method |
|---|---|---|
| display_board | up, down | FlapDisplayBlock.updateShape -> updatedShapeInner, updateColumn |
| metal_girder_encased_shaft | top, bottom | GirderEncasedShaftBlock.updateShape -> GirderBlock.updateVerticalProperty |
| gantry_shaft | part | GantryShaftBlock.updateShape |
| gantry_carriage | axis_along_first | GantryCarriageBlock.updateShape -> cycleAxisIfNecessary |
| large_water_wheel | extension | LargeWaterWheelBlock.updateShape |
| fluid_valve | enabled | FluidValveBlockEntity.tick, fluids phase |

ChainDriveBlock's CONNECTED_ALONG_FIRST_COORDINATE is serialized axis_along_first.
GantryCarriageBlock cycles the same serialized property. These shape outputs can
affect topology. Chain outputs now remain fully compared after committing
take_chain_shapes; gantry/wheel shape generation still requires its adapters. Masking properties does not make their missing topology
adapters supported, nor excuse speed/source/network discrepancies.

The remaining manifest-block updateShape overrides either return the state after
scheduling water (including inherited AbstractShaftBlock and HandCrankBlock), or
schedule subsystem work: WaterWheelBlock schedules a flow check; BeltBlock updates
tunnel connections and block-entity covered status, without changing its state.
PulleyBlock's waterlogged updateShape is on its nested rope class. Other kinetic
classes have no additional state-changing updateShape override. Placement methods
select facing, axis, ceiling, crafter pointing and shaft flags as input choices;
direct state injection supplies these, so those properties stay compared.
Kinetic generation remains dry; dedicated fluid cases observe contained native waterlogging.

FluidValveBlockEntity.onSpeedChanged chases its pointer toward 1 for positive speed
and toward 0 otherwise; tick changes enabled at those endpoints. The fluid driver now runs
the valve phase through tick_fluid_entities and compares enabled state as well as
kinetic observations. Pointer entity records cross the reload boundary.

Network counts read TorquePropagator.networks[level][entity.network].getSize().
KineticNetwork.getSize adds unloadedMembers to members.size. Reflection is confined
to the MIT registry field; a missing named network fails rather than constructing
one. Partitions still use block-entity network ids. Split-shaft member-count
differences remain findings: Java owns one KineticBlockEntity per split shaft.

Motor and controller kinetic-board selections now appear as `kinetic.speed_setting`
in both snapshots and are compared with the ordinary numeric tolerance, including
presence versus absence. Chain part and axis_along_first exclusions are removed.
Sequencer execution compares current/duration/progress/timer/previous-power and
block state. Valves compare selected angle, in-use ticks and handle angle.
Context is normalized as optional angle/distance effective limits; absence differs
from a present zero limit. For a sequencer's single Java entity, the outgoing
engine-half limit is scaled to the driven-half theoretical speed. No stale
context is suppressed when a run ends.

The engine has no public getters for its private chain signal or crank countdown.
Rust's chain_modifier is the engine formula applied to the last submitted signal;
valve in-use/angle comes from the glue-owned accepted-use record. Speed, generated
speed, networks and propagated context independently observe the engine output.
An independently readable private timer/setting would require an engine API
addition, outside this task's read-only engine boundary.

A valve externally driven while not in use delegates getIndependentAngle to a
client renderer. Calling that branch on a dedicated server fails class loading.
Both adapters explicitly emit angle_unavailable for that branch; they still
compare selected angle, in-use, every kinetic/network field and context. Active
player-use and resting undriven angles remain compared. No renderer is simulated.

Subsystem observations are selected by the current block type, as the glue's
sequencer message/load paths do. A cached sequencer record after replacement by
a different kinetic block is not that block's sequencer state. The replacement
regression retains this distinction without suppressing live sequencer context
or transition comparisons.

Belt observations add controller/index/length/casing and conveyor link offsets.
Kinetic part reconstruction supplies the current belt controller in Settings;
without it parts() returns no belt part even though the solver contains one.
This was a driver observation bug, corrected before classifying differences.
Items now have the input and observation contract in item-transport.md. Covered/color entity state still needs its own coverage.
