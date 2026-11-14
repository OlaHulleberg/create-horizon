# Validation

Run on 2026-11-14 (Linux x86_64, Rust 1.99.1 as the SDK pins, the mod pinned to the fork build
`42c5f703e3a0414297c1e1f00d62d4d4b4b2e580`, `cargo-horizon-mod` built from the fork at that commit).

| Check | Result |
|---|---|
| `cargo +1.99.1 fmt --check` (in `create/`) | Pass |
| `cargo +1.99.1 clippy --all-targets -- -D warnings` (in `create/`) | Pass |
| `cargo +1.99.1 test` (in `create/`) | 16 glue tests pass |
| `cargo +1.99.1 test -p create-engine` | 533 engine tests pass |
| `cargo horizon-mod check` (in `create/`) | Pass: the fork's package rules, the declarations against its content schema, `cargo check` against the pinned SDK. "Create uses 2266 items of the API … Made for 42c5f70." |
| `cargo horizon-mod build` (in `create/`) | Pass: `create/target/horizon-mod/create.hmod` (sources and declarations, no Create assets) |
| `cargo test`, `clippy -D warnings`, `fmt --check` in `tools/create-data` | Pass (21 tests) |
| `cargo horizon-mod fits --build 0cad862` | Does not fit: the API changed between those builds |

## Not run

- **In game.** Nothing of the port has been played on the mod API yet. The current Nightly
  (514ecf9) publishes no SDK and no prebuilt game exists for the pinned build, so the launcher
  cannot build it for a player today; it can once a Nightly at or after 42c5f70 ships with its SDK
  (`cargo horizon-mod update` moves the pin), or on a developer base built from the fork at the
  pin (`docs/modding/src/how-to/native-mods/07-local-developer-builds.md`). The in-game proofs and
  benchmarks in PORTING.md and `benchmarks/` are the earlier mixin-based port's.
- **`create-data compile` against Create's sources.** The declarations were converted to the fork's
  content schema (typed `state_properties` without the template's own, `default_state`, shape
  tables over `properties` covering every template value, upper-case `render_shape`, fluids'
  `world`) by the same rules `tools/create-data` now applies, checked by its unit tests; they were
  not regenerated from a Create checkout, since upstream has no 6.0.11 tag and the earlier port's
  source commit is not recorded. The fork's loader checks a template's own properties only when
  the game loads, so the vanilla template properties the generator assumes
  (`TEMPLATE_PROPERTIES`) are verified first in game.
