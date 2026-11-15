# Contributing

Issues and pull requests are welcome. Bug reports help most with steps to reproduce, the Sunrise build and the
game log.

## Before you start

Read [docs/architecture.md](docs/architecture.md). Rules go in the engine, the glue only uses `mod_api`, and
missing API goes in [API_GAPS.md](API_GAPS.md) instead of a workaround.

## Never commit Create's assets

Create's `src/main/resources/assets` is All Rights Reserved. Don't commit it or anything copied or adapted from it
(textures, models, block states, particles, sounds, translations), nor Create's jar. Name assets by id; what needs
them is built by `create-data prepare` on the player's machine. A test in `tools/create-data` enforces this.
Create's English UI text (`src/generated/resources/assets/create/lang/en_us.json`) is MIT and may be used.

Also don't commit binaries or `create/.horizon/`.

## Checks

Run the checks listed in [VALIDATION.md](VALIDATION.md). In the pull request, say what you ran and what you tried in
game.

## Sign-off (DCO)

Commit with `git commit -s`: the `Signed-off-by` line certifies the
[Developer Certificate of Origin](https://developercertificate.org).

## AI-assisted pull requests

Say so in the description if AI tools helped. You are responsible for every line, and must have read and tested it.
