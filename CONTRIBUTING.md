# Contributing

Issues and pull requests are welcome. Bug reports help most with the steps to reproduce, the Sunrise
build, and the game log.

## Before you start

Read [notes/native-port.md](notes/native-port.md). In short:

- Gameplay rules go in the engine (`create/src/engine`): plain Rust with no game types, tested with
  `cargo test`. The glue (`create/src/glue`) reads the world through `mod_api`, calls the engine and
  writes back.
- Use only `mod_api` and `create_engine`: no game internals, no `[[native.mixin]]`, no registry
  dependencies.
- If the API can't do something, leave it out and say so in the module's doc and in
  [HORIZON_API.md](HORIZON_API.md). No stubs or shims.
- Generated files (`create/src/engine/src/generated.rs`, `create/data/`, `create/assets/`) come from
  `tools/create-data compile`. Don't edit them by hand.

## Never commit Create's assets

Create's `src/main/resources/assets` is All Rights Reserved. Never commit it, or anything copied or
adapted from it: textures, models, block states, particle files, sounds or translations. Name them by
id instead. Whatever must contain them is produced by `create-data prepare` from the player's own jar.
A test in `tools/create-data` allows only generated block states and models in `create/assets`.

English UI text from Create's `src/generated/resources/assets/create/lang/en_us.json` is MIT and may be
used.

Also don't commit binaries, `create/.horizon/` (the fetched SDK) or any Minecraft files.

## Checks

Run these before opening a pull request:

```sh
cd create
cargo +1.99.1 fmt --check
cargo +1.99.1 clippy --all-targets -- -D warnings
cargo +1.99.1 test
cargo +1.99.1 test -p create-engine
cargo horizon-mod check
cd ../tools/create-data && cargo test
```

In the pull request, say what you ran, and what you tried in game (or that you didn't).

## Sign-off (DCO)

Every commit needs a `Signed-off-by` line: commit with `git commit -s`. It states that you wrote the
change, or have the right to submit it, under the
[Developer Certificate of Origin](https://developercertificate.org).

## AI-assisted pull requests

If AI tools helped write a pull request, say so in its description. You are responsible for every line
and must have read and tested it.
