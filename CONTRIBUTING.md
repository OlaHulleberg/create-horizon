# Contributing

Issues and pull requests are welcome. Bug reports help most with steps to reproduce, the Sunrise build and the
game log.

## Before you start

Read [docs/architecture.md](docs/architecture.md). Rules go in the engine, the glue only uses `mod_api`, and
missing API goes in [API_GAPS.md](API_GAPS.md) instead of a workaround.

## Never commit Create's assets

Create's `src/main/resources/assets` is All Rights Reserved. Don't commit it or anything copied or adapted from it
(textures, models, block states, particles, sounds, translations), nor Create's jar. Name assets by id; what needs
them is built by `create-data prepare` on the player's machine; draw Create's models by id and sample its textures
from the atlas, never write their geometry, UVs or pixel values into code. Create's English UI text
(`src/generated/resources/assets/create/lang/en_us.json`) is MIT and may be used.

A test in `tools/create-data` (`committed_assets`) allows only the generated block states and block models and
Catnip's placement indicator under `create/assets`, and, where Create's jar is at hand (`$CREATE_JAR` or the
`prepare` cache), refuses any file under `create/assets` or `create/data` that is one of the jar's assets, byte for
byte or as JSON. Code that copies a model's numbers is for review to catch.

Also don't commit binaries or `create/.horizon/`.

## Keep it MIT

Derive behaviour from Create's MIT code and data, vanilla's data in the game, or first principles. Never read or copy
NeoForge's source or data (LGPL), horizon-rust/horizon at or after `fb1135c6` (LGPL), or code and tables of
Sunrise's own (read it only to learn the API).

## Checks

Run the checks listed in [VALIDATION.md](VALIDATION.md). In the pull request, say what you ran and what you tried in
game.

## Sign-off (DCO)

Commit with `git commit -s`: the `Signed-off-by` line certifies the
[Developer Certificate of Origin](https://developercertificate.org).

## AI-assisted pull requests

Say so in the description if AI tools helped. You are responsible for every line, and must have read and tested it.
