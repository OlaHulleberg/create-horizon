# Create for Horizon

A port of [Create](https://github.com/Creators-of-Create/Create) 6.0.11 to Horizon (Minecraft 26.3 in Rust), as a
native mod on the mod API of the [arismoko/horizon](https://github.com/arismoko/horizon) fork: hooks, Parts, block
entities, attachments, capabilities under transactions, payloads and draw contexts. It depends only on `mod_api`:
no game internals, no mixins, and no registry dependencies. No Java runs, at build time or play time.

The mod pins one exact fork build in `create/horizon-mod.toml` (`[native] horizon`). The launcher compiles it
against that build's prebuilt game; nothing of the game is rebuilt.

**Status:** every legacy subsystem is ported to the API; the mod passes the fork's package check and builds against
its pinned SDK, with the engine's and the glue's unit tests passing. It has not yet been played on this API, and the
launcher can build it for players once a Nightly at or after the pinned build ships with its SDK: see
[VALIDATION.md](VALIDATION.md).
What the API cannot do yet, and so what the port leaves out, is listed in [HORIZON_API.md](HORIZON_API.md).

## Layout

```text
create/                        The native mod: what the launcher builds
  horizon-mod.toml             Identity and the pinned fork build
  Cargo.toml                   The mod's crate (only the pinned SDK and the engine)
  src/lib.rs                   The one `impl Hooks`: each hook handed to every subsystem
  src/glue/                    The game glue on the mod API, one module per subsystem
  src/engine/                  create-engine: Create's rules as plain Rust with no game types
    src/generated.rs           @generated block table (tools/create-data compile)
  data/create/horizon/         @generated block, item and fluid declarations
  assets/create/               @generated block states of the partial models
tools/create-data/             compile: Create's MIT data -> tables; prepare: Create's jar -> playable mod
notes/                         Porting notes (start with notes/native-port.md)
benchmarks/, PORTING.md        Measurements and the porting log of the earlier mixin-based port
```

`create/src/glue/` holds the core (`states`, `world`, `server`, `persist`, `net`, `subsystem`) and the subsystems:
kinetics, logistics, processing, fluids, equipment and gauges, redstone and display, contraptions, trains and carts,
packages, stock and schematics, interaction and items, decor, blocks and copycats.
[notes/native-port.md](notes/native-port.md) explains how they fit together.

## Licences

Create's code and data (everything outside its `src/main/resources/assets/`) are MIT and are read, translated and
compiled here; its assets are All Rights Reserved and never committed or shipped. The player's own machine downloads
Create's official jar from Modrinth, where the Create team publishes it, checked against the SHA-512 pinned here: the
launcher does it for the `[[jar]]` in `create/horizon-mod.toml` and layers the jar's assets and data under the mod,
and `create-data prepare` does it into the user's cache to make what 26.3 needs from them. See [LICENSE](LICENSE) and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Build and play

Requirements: the fork's launcher (`horizon-launcher`) and its `cargo horizon-mod` tool (`cargo install --locked --git
https://github.com/sunrise-org/sunrise --rev <the pin> cargo-horizon-mod`), Rust 1.99.1 with the `wasm32-unknown-unknown`
target (the SDK's toolchain). Create's jar (1.21.1-6.0.10, the newest published; 6.0.11 is not released) is
downloaded for you.

```sh
cd create && cargo horizon-mod check                     # fetches the pinned SDK into create/.horizon/, checks
cd tools/create-data && cargo run -- prepare --out ../../target/create-mod/create
horizon-launcher native-build target/create-mod --instance "My Instance" --trust-native-code
horizon-launcher play --instance "My Instance" --allow-jar-downloads
```

`prepare` downloads Create's jar from Modrinth's CDN into your cache folder (`$CREATE_DATA_CACHE`, else
`~/.cache/create-horizon/`), checks its SHA-512, and assembles the mod folder: this repository's crate and
declarations, Create's data converted from 1.21.1 to 26.3, and what 26.3 needs made from Create's assets (item
model definitions, flattened and tinted models, connected-texture tiles). Offline, give it the jar:
`prepare --out <folder> --jar /path/to/create-1.21.1-6.0.10.jar`. On first play the launcher fetches the same jar
into the instance's `jars/` (the launcher's window asks first; on the command line `--allow-jar-downloads` says yes).
The folder holds what is made from Create's assets: it is for your own game, never for redistribution.

After a Create data change, regenerate the committed tables and declarations from a Create checkout:

```sh
cd tools/create-data && cargo run -- compile --create /path/to/Create --out ../..
```

## Validation

```sh
cd create
cargo +1.99.1 fmt --check
cargo +1.99.1 clippy --all-targets -- -D warnings
cargo +1.99.1 test                      # the glue's tests
cargo +1.99.1 test -p create-engine     # the engine's tests
cargo horizon-mod check                 # the fork's package rules, then cargo check for the pinned build
cd ../tools/create-data && cargo test --locked
```
