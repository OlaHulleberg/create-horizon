# Validation

Run on 2026-11-16 (Linux x86_64, Rust 1.99.1), the mod pinned to Sunrise `b77574eb1b7ef6b8a36456ec890a3d2ba3af37a6`.
Sunrise has published no SDK for this pin yet, so `cargo horizon-mod check` used `cargo-horizon-mod` built from
Sunrise at the pin with an SDK built from the same checkout, signed with a throwaway key and fetched through
`HORIZON_SDK_BASE` / `HORIZON_SDK_TRUSTED_KEY`.

## Checks

| Where | Command | Result |
|---|---|---|
| `create/` | `cargo +1.99.1 fmt --check` | Pass |
| `create/` | `cargo +1.99.1 clippy --all-targets -- -D warnings` | Pass |
| `create/` | `cargo +1.99.1 test` | Pass (77 glue tests) |
| `create/` | `cargo +1.99.1 test -p create-engine` | Pass (655 engine tests) |
| `create/` | `cargo horizon-mod check` | Pass ("Create uses 3565 items of the API … Made for b77574e.") |
| `create/src/engine/` | `cargo +1.99.1 fmt --check` | Pass |
| `create/src/engine/` | `cargo +1.99.1 clippy --all-targets -- -D warnings` | Pass |
| `tools/create-data/` | `cargo +1.99.1 fmt --check` | Pass |
| `tools/create-data/` | `cargo +1.99.1 clippy --locked --all-targets -- -D warnings` | Pass |
| `tools/create-data/` | `cargo +1.99.1 test --locked` | Pass (37 tests, `committed_assets` included) |

## In game

A local developer base built from Sunrise at the pin, the mod built by the launcher's `native-build` from
`create-data prepare` of Create 1.21.1-6.0.10's jar, a fresh creative world:

- A creative motor drives shafts through a gearbox into a cogwheel (speeds 16, -16, 16), drawn turning.
- A belt laid with the belt connector carries dropped items along and off its end.
- A press over a depot turns an iron ingot into an iron sheet.
- A mixer at 32 rpm over a basin makes mushroom stew from a bowl and two mushrooms (it waits at 16 rpm, below
  Create's medium speed).
- A pump moves a water source through pipes into a fluid tank (1000 mB).
- A mechanical bearing with a glued drill assembles on an empty-hand click, turns, hurts a pig in the drill's path
  (about 3.7 per pass) until it dies, and comes apart on another click.
- A train sent from one station to another waits before a red (powered) signal and goes on when it turns green.
- Signals placed with the item's targeting flow (pick the track, then place within 16 blocks, on either side and
  not only beside the track) govern the trains running the way the player looked; a one-sided signal refuses
  trains the other way, two sides make a dual signal; markers point the governed way; the pick survives a save
  and reload. A signal with no pick (a command, an older save) binds as before.
- Advancement toasts show (Train Signal, Shifting Gears).
- The schematic and quill draws its selection box.
- The game log has no errors, panics or refused content. Warnings: vanilla damage-type tags with unresolved
  entries (Create's damage types aren't registered, see [API_GAPS.md](API_GAPS.md)) and the jar's copies of
  files the mod also ships.

## Not run

- **A player's build.** Without a published SDK for the pin, the launcher builds the mod only on a developer base
  built from Sunrise at the pin, and CI's `cargo horizon-mod check` fails at the SDK fetch.
- **`create-data compile` against Create's sources.** The committed declarations were not regenerated from a
  Create checkout this cycle.
