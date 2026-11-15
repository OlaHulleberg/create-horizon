# Working on the Create native mod

Read README.md and notes/native-port.md before editing. The mod is `create/`: a native mod on the
mod API of Sunrise (sunrise-org/sunrise, the arismoko/horizon fork's successor), pinned to one
Sunrise build in `create/horizon-mod.toml`. Keep this repository separate from Horizon; never modify a Horizon checkout or player resources. Do not commit
binaries, `.horizon/` (the fetched SDK) or Minecraft assets.

Gameplay rules live in the create-engine crate (`create/src/engine`), plain Rust with no game types,
unit-tested with cargo test. The glue (`create/src/glue`) only reads the world through `mod_api`,
calls the engine and writes back; `create/src/lib.rs` is the one `impl Hooks`, handing each hook to
every subsystem. Per-block, per-item and per-block-entity moments are Parts the subsystem attaches
to its own blocks. Only `mod_api` and `create_engine`: no game internals, no `[[native.mixin]]`, no
registry dependencies. Where the API cannot do something, leave it out, say so in the module's doc
and in HORIZON_API.md; no stubs or shims.

Generated files (`create/src/engine/src/generated.rs`, `create/data/`, `create/assets/`) come from
`tools/create-data compile`: never edit them by hand. Never commit Create's
`src/main/resources/assets` (All Rights Reserved) or anything copied or adapted from it: textures,
models, block states, particle files, sounds or translations. Name them by id instead; what must
contain them is produced by `create-data prepare` from the player's jar (a test in
`tools/create-data` allows only generated block states and models in `create/assets`). English UI
text from Create's `src/generated/resources/assets/create/lang/en_us.json` is MIT.

Validate in `create/` with the SDK's toolchain: `cargo +1.99.1 fmt --check`, `cargo +1.99.1 clippy
--all-targets -- -D warnings`, `cargo +1.99.1 test`, `cargo +1.99.1 test -p create-engine`, and
`cargo horizon-mod check`; in `tools/create-data`, `cargo test`. Keep validation claims honest:
say what was run, and what was not run in game.
