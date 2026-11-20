# Contributing

Issues and pull requests are welcome. Bug reports help most with steps to reproduce, the Sunrise build and the
game log.

## Before you start

Read [docs/architecture.md](docs/architecture.md). Rules go in the engine, the glue only uses `mod_api`, and
missing API goes in [API_GAPS.md](API_GAPS.md) instead of a workaround.

## Never commit Create's assets

Create's `src/main/resources/assets` is All Rights Reserved. Don't commit it or anything copied or adapted from it
(textures, models, block states, particles, sounds, translations), nor Create's jar. The game layers the
player's jar under the mod. Draw Create's models by id and sample its textures from the atlas; never write its
geometry, UVs or pixel values into code. Where references are not enough, `create-data prepare` builds the
remaining assets locally. Create's English UI text
(`src/generated/resources/assets/create/lang/en_us.json`) is MIT and may be used.

A test in `tools/create-data` (`committed_assets`) allows generated block states and models, verified item
references, the port's connected-texture rules, atlas declarations and shader, and Catnip's placement indicator under
`create/assets`. When Create's jar is at hand
(`$CREATE_JAR` or the download cache), it refuses any file under `create/assets` or `create/data` that copies one of
the jar's assets, byte for byte or as JSON. Code that copies a model's numbers is for review to catch.

Also don't commit binaries or `create/.sunrise/`.

## Keep it MIT

Derive behaviour from Create's MIT code and data, vanilla's data in the game, or first principles. Never read or copy
NeoForge's source or data (LGPL), horizon-rust/horizon at or after `fb1135c6` (LGPL), or code and tables of
Sunrise's own (read it only to learn the API).

`tools/create-fuzz`'s Java harness builds against NeoForge, Minecraft and Create as Gradle dependencies, to run
Create for comparison. Use only their public APIs, and never commit their jars, the built harness or anything
from their sources.

## Regenerate registry overlays

From `tools/create-data/`, run `cargo run --locked -- registry --out ../../create`. This downloads the pinned jar
and the pinned `src/generated` tree metadata. The generator matches data and referenced item models to MIT source
blobs and writes the converted data, omissions and plain item references. It also writes the reference-only
coupling, steam-frame, animated tool and connected-texture declarations and the port-owned chain models and shader. `--check`
compares without writing; `--audit <file>` records the evidence for each item definition accepted or refused.
For an offline run, supply
`--jar <pinned jar>` and `--tree <saved GitHub tree JSON>`. Keep the output separate from local `prepare` assets;
never commit the latter. `compile` still owns the startup declarations and top-level advancements.

## Checks

Use `cargo sunrise-mod` for the pinned SDK. Run the checks listed in [VALIDATION.md](VALIDATION.md). In the pull request, say what you ran and what you tried in
game.

## Sign-off (DCO)

Commit with `git commit -s`: the `Signed-off-by` line certifies the
[Developer Certificate of Origin](https://developercertificate.org).

## AI-assisted pull requests

Say so in the description if AI tools helped. You are responsible for every line, and must have read and tested it.
