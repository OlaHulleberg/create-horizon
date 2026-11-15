# Validation

Run on 2026-11-15 (Linux x86_64, Rust 1.99.1 as the SDK pins), the mod pinned to the Sunrise build
`b77574eb1b7ef6b8a36456ec890a3d2ba3af37a6` (sunrise-org/sunrise main, which carries arismoko/horizon's
`de8b4e2526` mod API unchanged), `cargo-horizon-mod` built from Sunrise at that commit. No SDK is published for
it yet: the SDK was built from that checkout (`tools/prebuilt_game.py build` and `pack`,
`tools/sdk_api_source.py`), signed with a throwaway key and fetched through `HORIZON_SDK_BASE` and
`HORIZON_SDK_TRUSTED_KEY` (a debug build of the tool), as the fork's daily check does.

| Check | Result |
|---|---|
| `cargo +1.99.1 fmt --check` (in `create/`) | Pass |
| `cargo +1.99.1 clippy --all-targets -- -D warnings` (in `create/`) | Pass |
| `cargo +1.99.1 test` (in `create/`) | 65 glue tests pass |
| `cargo +1.99.1 test -p create-engine` | 643 engine tests pass |
| `cargo horizon-mod check` (in `create/`) | Pass: "Create uses 3277 items of the API … Made for b77574e." |
| `cargo test --locked`, `clippy --locked -D warnings`, `fmt --check` in `tools/create-data` | Pass (34 tests, `committed_assets` included) |

## In game

On a local unsigned developer base built from Sunrise at the pin, the mod built by the launcher's
`native-build` (`create-data prepare` of Create 1.21.1-6.0.10's jar), in a fresh creative world:

- The game log has no errors, panics or refused content (an earlier build stopped at load: the game now
  refuses a recipe file whose serializer nobody registered, so the mod registers Create's).
- A creative motor drives shafts through a gearbox into a cogwheel and a crossing shaft (speeds 16, -16, 16).
- A belt made with the belt connector between two shafts carries dropped items along it and off its end.
- A mechanical press over a depot turns iron ingots into iron sheets.
- A mechanical pump driven through a cogwheel pulls a water source through pipes into a fluid tank (1000 mB).
- A mechanical bearing assembles the block on it and turns it.

Trains were not tried.

## Not run

- **A player's build.** No SDK release exists for the pin, so a launcher can build the mod only on a
  developer base built from Sunrise at the pin
  (`docs/modding/src/how-to/native-mods/07-local-developer-builds.md`), until Sunrise publishes that
  build's SDK.
- **`create-data compile` against Create's sources.** The declarations were converted to the fork's
  content schema (typed `state_properties` without the template's own, `default_state`, shape
  tables over `properties` covering every template value, upper-case `render_shape`, fluids'
  `world`) by the same rules `tools/create-data` now applies, checked by its unit tests; they were
  not regenerated from a Create checkout, since upstream has no 6.0.11 tag and the earlier port's
  source commit is not recorded. The fork's loader checks a template's own properties only when
  the game loads, so the vanilla template properties the generator assumes
  (`TEMPLATE_PROPERTIES`) are verified first in game.
