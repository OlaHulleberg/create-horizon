# Java behaviour comparisons

These development tests compare Create 6.0.11's Java methods with this port's public
`create-engine` functions. They transfer input matrices from
[Delete228/create-horizon](https://github.com/Delete228/create-horizon), originally written
against Create 6.0.10. The Java reference is now pinned to
[`37ea4af`](https://github.com/Creators-of-Create/Create/commit/37ea4afcfe5bf85875d4b0749798a45330070a1f).

This is a partial migration. The [migration ledger](MIGRATION.md) identifies every original
reference program, the observations transferred here and the remaining work. A passing
isolated method comparison does not establish world, save, UI, rendering or whole-mod parity.

## Run

You need Rust 1.99.1, Python 3.9 or newer, a JDK 21 or newer and a Git checkout containing
the exact Create commit in [reference.json](reference.json). No Minecraft installation,
Create jar, mod SDK, Gradle, account or game launch is required for these comparisons.
The runner reads the checkout with `git show`; it does not change its branch or files.

```sh
python3 -m unittest discover -s tests/java-parity -p 'test_*.py' -v
python3 tests/java-parity/run.py --create-source /path/to/Create
```

The default command writes `target/java-parity/report.json` and `report.md`. It exits 0 only
when **every requested observation** matches, 1 for differences/unavailable observations or
engine panics, and 2 for compilation, extraction, adapter or malformed-stream errors.
The current full comparison intentionally exits 1; see [measured results](RESULTS.md).

Select individual suites with repeatable `--suite` arguments. For example:

```sh
python3 tests/java-parity/run.py --create-source /path/to/Create \
  --suite placement --suite rotation --suite network --suite transport-processing
```

`--report-only` runs the same comparisons and records the same differences, but does not fail
on a behavioural difference or unavailable observation. Harness errors still fail. This mode
is explicitly observational; success is not a parity certificate. `--probe /path/to/binary`
can reuse an already built probe during local investigation.

The standalone Cargo package under `probe/` has its own workspace and lockfile. Its only
dependency is this checkout's `create-engine`. It is not a mod dependency, workspace member,
build script, feature or part of the shipped binary. Generated Java, compiled classes, Rust
output and reports stay in the ignored `target/` directory. Nothing installs into player data.

## Observations

The Java AST exporter selects a single method body by owner, first parameter or arity. The
runner inserts that **unchanged body** into an authored fixture context and compiles it.
`KineticNetwork` is compiled as a complete original class with explicit getter contexts.
Source and extracted-body hashes are written to the report alongside the Java and subject
revisions. Generated original source is never committed.
The provenance test also checks 14 transferred input streams byte for byte against hashes
recorded from the original generators at the origin commit; it does not use Java results
as stored goldens. The belt surface observation is a subset of the old rendering recorder.

The Rust probe calls the actual public engine. It contains fixture inputs, callback contexts
and observation formatting; it does not embed the old Rust port or translate the Java rule
under test. Missing observations are reported as unavailable rather than supplied by a
second implementation. The original [MIT notice](ORIGIN_LICENSE) is preserved.

| Suite | Observed contract / limits |
|---|---|
| `network` | Total stress and capacity in 75 explicit loaded-member contexts. Not full propagation or persistence. |
| `placement` | Shaft, motor and cog placement decisions for exact look/click/shift/neighbor inputs. |
| `rotation` | Local ratios and source-relative shaft modifiers. Not network scheduling. |
| `transport` | Connector layout, movement, side insertion and inventory tick histories. Synthetic funnel ending, read-only stack lookup and ejection velocity are unavailable. World callbacks are controlled fixtures. |
| `transport-processing` | Strict handler distance, callback counts, change notifications and effective saved item multiset. Queue identity/order is deliberately normalized. Fixtures classify no-op/equal outputs before returning the public `Process` value. |
| `belt-surface` | Fallback Java renderer's surface names, shift flag and normalized UV phase. Transform calls are unobserved fixture operations; no claim about poses, shaders, instancing, assets or pixels. |
| `processing` | Press speed and head curve. Bulk held-item recipe routing is unavailable. |
| `milling` | Millstone speed/timer/inventory histories. The fixture handler limit is 99 and item stack limit is 64. Seeded result counts are compared, but PRNG draw counters are not; the engine deliberately uses a different PRNG. |
| `drilling` | Damage and breaker histories with explicit hardness/world inputs. The fixture supplies scheduled wake events. The isolated private break-speed expression is unavailable. |
| `visual` | Phase offset, speed gauge target and water-wheel generation. Private renderer time-angle and crank interpolation paths are unavailable. |
| `fan` | Full transported/dropped processing timer histories, range, direction and propeller speed with explicit recipe/NBT facts. |
| `fan-media` | Catalyst validity masks and priority. Haunting/smoking custom fluid tags cannot be represented by the public `Catalyst` input and are unavailable. |
| `fan-entity` | Motion vectors for eligible entities in the authored context. Entity effects/selection, fall and flight counters are outside this public operation. |
| `burner` | Accepted fuel, remaining time and creative state. Fuel resolution, heat publication, notification counts and effects are excluded. Simulation uses a private state copy. Integer-boundary panics are recorded, not hidden. |
| `basin` | Stationary recipe success and inventory/fluid state for representable capabilities, plus mixer server histories. Routing, absent capabilities, scripting the concrete PRNG and removal publication are unavailable. Host draw/notification counters are excluded. |
| `motion-primitives` | Supplemental active windmill speed, rope range and server linear/piston/gantry speed checks. No assembly, save/reload, client corrections, sequenced limits or entity collisions. |

Integer states, raw float bits and timer histories are exact. Tolerances apply only to named
float observations in `specs.py`; the belt recorder compares surface names and flags exactly.
NaN does not pass a tolerant comparison. Both output streams must have exactly one nonempty
observation per input. Fixture/probe errors fail the harness; only a panic originating in the
engine source is classified as an engine panic. Samples are bounded **per outcome**, so a long
list of unavailable cases cannot hide a later difference.

## CI

[java-parity.yml](../../.github/workflows/java-parity.yml) is a separate workflow. It runs the
harness tests, probe formatting/lints and the full observational comparison. Reports are
uploaded and shown in the workflow summary. Five fully supported regression suites
(`network`, `placement`, `rotation`, `transport-processing`, `motion-primitives`) also gate the job: every case
must pass. Other suites remain observations while their differences and adapter gaps are
reviewed. The normal mod workflow and its SDK/build dependencies are unchanged.

For live comparisons, the existing separate
[create-fuzz](https://github.com/OlaHulleberg/create-fuzz) project already runs Java and the
Rust engine in paired scenarios. This package complements it with deterministic method
matrices; it does not replace its world adapter. Porting the remaining original live tests
requires paired observations in that infrastructure or a separate SDK test host, not copies
of the old mixin port's private RPC/NBT assumptions.
