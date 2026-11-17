# Issue 18: adjustable chain gearshift signal changes

The version 3 issue case was run against Create's shared Java server with checkpoints immediately
before and after every signal change (38/39, 42/43, …, 118/119). Each observation through
checkpoint 118 has speed 32, source (-1,8,0), network size 2. Checkpoint 119 is air; the motor
remains at 32 with network size 1. The breaking action is signal 3 → 11 during tick 118.
No gameplay verification was performed, as requested by the brief.

The deciding guard is `RotationPropagator.propagateNewSource`:

```java
boolean speedChangedTooOften = currentTE.getFlickerScore() > MAX_FLICKER_SCORE;
if (tooFast || speedChangedTooOften) {
    world.destroyBlock(pos, true);
    return;
}
```

`MAX_FLICKER_SCORE` is 128. `ChainGearshiftBlockEntity.analogSignalChanged` says:

```java
detachKinetics();
removeSource();
signal = newSignal;
attachKinetics();
```

`KineticBlockEntity.onSpeedChanged` adds five for each start or stop, and its base tick
subtracts one before `ChainGearshiftBlockEntity.tick` processes the changed signal. The first
signal change produces tally 10; subsequent changes four ticks apart add a net six. At tick
114 the tally after restart is 124; at tick 118 it is 130 and the recursive propagation guard
breaks the gearshift. Speeds are always 32, with no opposing direction or inconsistent cycle:
the sole other kinetic entity is the motor, connected at a 1:1 shaft face.

`ChainGearshiftBlock.neighborChanged` also changes POWERED using
`Block.UPDATE_CLIENTS | Block.UPDATE_KNOWN_SHAPE`. This skips
`KineticBlock.updateIndirectNeighbourShapes`, whose normal path calls
`clearKineticInformation`. Treating this notification as an ordinary state replacement
silently cleared speed before the analog handler, losing start/stop flicker. Both distinctions
matter: preserve state for this commit, and process the analog lifecycle after the base tick.

The engine now exposes `CreateWorld::setting_known_shape(dim, pos, new_state)` for a
same-block commit using UPDATE_KNOWN_SHAPE, called before the normal block-change notification.
The glue computes the powered state, calls that method, and writes with CLIENTS | KNOWN_SHAPE.
Analog inputs queue on the kinetic entity and run detach/removeSource/install/attach after the
base attachment and flicker decay. `set_chain_signal` no longer routes through generic settings.

## create-fuzz driver update required

In `rust/src/engine.rs`, in `tick_inputs`, immediately before
`self.commit(p, id, false)`, after `transmission_power_changing`, call:

```rust
if self.w.reads_signal(state) {
    self.w.setting_known_shape(0, p, id);
}
```

Keep `set_chain_signal` in its existing position. The shared create-fuzz checkout was not
edited. An isolated copy of its driver under `/mnt/lanes/agent-18/adapter-check` used this call
for validation. All 42 remaining differences in the expanded checkpoint case are the
`axis_along_first` default (#17); kinetic observations and destruction timing match.
The unmodified driver cannot validate this changed glue contract.

`world::tests::analog_chain_sweep_breaks_on_tick_118` reconstructs the observed motor,
gearshift and signal schedule, asserting 32 rpm until the break at exactly tick 118.
It passes with the fix. With all three engine files restored to main and only the test plus a
no-op declaration of the new interface retained, it fails at tick 118: no break occurs.
Disabling just known-shape preservation also fails (break at tick 102), confirming that merely
adding the analog lifecycle is insufficient.

## Validation

Engine: fmt --check, clippy --all-targets -- -D warnings, and all 688 tests pass.
Glue: fmt --check, clippy --all-targets -- -D warnings, and all 84 tests pass.
The SDK/package check passes. create-data fmt, locked clippy and all 46 locked tests
(including committed_assets) pass. All Cargo commands used Rust 1.99.1 and four build jobs.

Both full sweeps used the current create-fuzz main version 3 generator and the unmodified
shared driver. This generator has existing differences beyond the brief's historical
10299/1 baseline.

Main: 9013 matched, 1287 differed, 0 java-unstable.

Branch: 9334 matched, 966 differed, 0 java-unstable.

New mismatches: [].

Every seed changing from differed on main to matched on this branch (321):

44, 60, 108, 124, 172, 188, 236, 252, 300, 316, 364, 380, 428, 444, 492, 508, 556, 572, 620, 636, 684, 700, 748, 764, 812, 828, 876, 892, 940, 956, 1004, 1020, 1068, 1084, 1132, 1148, 1196, 1212, 1260, 1276, 1324, 1340, 1388, 1404, 1452, 1468, 1516, 1532, 1580, 1596, 1644, 1660, 1708, 1724, 1772, 1788, 1836, 1852, 1900, 1916, 1964, 1980, 2028, 2044, 2092, 2108, 2156, 2172, 2220, 2236, 2284, 2300, 2348, 2364, 2412, 2428, 2476, 2492, 2540, 2556, 2604, 2620, 2668, 2684, 2732, 2748, 2796, 2812, 2860, 2876, 2924, 2940, 2988, 3004, 3052, 3068, 3116, 3132, 3180, 3196, 3244, 3260, 3308, 3324, 3372, 3388, 3436, 3452, 3500, 3516, 3564, 3580, 3628, 3644, 3692, 3708, 3756, 3772, 3820, 3836, 3884, 3900, 3948, 3964, 4012, 4028, 4076, 4092, 4140, 4156, 4204, 4220, 4268, 4284, 4332, 4348, 4396, 4412, 4460, 4476, 4524, 4540, 4588, 4604, 4652, 4668, 4716, 4732, 4780, 4796, 4844, 4860, 4908, 4924, 4972, 4988, 5036, 5052, 5100, 5116, 5164, 5180, 5228, 5244, 5292, 5308, 5356, 5372, 5420, 5436, 5484, 5500, 5548, 5564, 5612, 5628, 5676, 5692, 5740, 5756, 5804, 5820, 5868, 5884, 5932, 5948, 5996, 6012, 6060, 6076, 6124, 6140, 6188, 6204, 6252, 6268, 6316, 6332, 6380, 6396, 6444, 6460, 6508, 6524, 6572, 6588, 6636, 6652, 6700, 6716, 6764, 6780, 6828, 6844, 6892, 6908, 6956, 6972, 7020, 7036, 7084, 7100, 7148, 7164, 7212, 7228, 7276, 7292, 7340, 7356, 7404, 7420, 7468, 7484, 7532, 7548, 7596, 7612, 7660, 7676, 7724, 7740, 7788, 7804, 7852, 7868, 7916, 7932, 7980, 7996, 8044, 8060, 8108, 8124, 8172, 8188, 8236, 8252, 8300, 8316, 8364, 8380, 8428, 8444, 8492, 8508, 8556, 8572, 8620, 8636, 8684, 8700, 8748, 8764, 8812, 8828, 8876, 8892, 8940, 8956, 9004, 9020, 9068, 9084, 9132, 9148, 9196, 9212, 9260, 9276, 9324, 9340, 9388, 9404, 9452, 9468, 9516, 9532, 9580, 9596, 9644, 9660, 9708, 9724, 9772, 9788, 9836, 9852, 9900, 9916, 9964, 9980, 10028, 10044, 10092, 10108, 10156, 10172, 10220, 10236, 10284.
