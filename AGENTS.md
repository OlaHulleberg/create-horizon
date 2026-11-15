# Working on the Create native mod

Read [README.md](README.md) and [docs/architecture.md](docs/architecture.md) first.

- The mod is `create/`, built on Sunrise's mod SDK at the build pinned in `create/horizon-mod.toml`.
- Rules go in the engine (`create/src/engine`); the glue (`create/src/glue`) only reads the world, calls the
  engine and writes back. Only `mod_api` and `create_engine`; no game internals or mixins.
- If the API can't do something, leave it out and record it in [API_GAPS.md](API_GAPS.md). No stubs or shims.
- Never edit generated files by hand (`create/src/engine/src/generated.rs`, `create/data/`, `create/assets/`); they
  come from `tools/create-data compile`.
- Never commit Create's assets (All Rights Reserved), its jar or anything extracted from it, binaries, or
  `create/.horizon/`. Details: [CONTRIBUTING.md](CONTRIBUTING.md).
- Run the checks in [VALIDATION.md](VALIDATION.md) and report honestly what was run and what was tried in game.
