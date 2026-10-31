# Working on a native Horizon mod

Read README.md and compatibility.json before editing. Keep this repository
separate from Horizon and horizon-mixins. Only export committed pinned Horizon
sources into ignored target trees; never modify the original checkout or player
resources. Do not commit binaries, exported sources or Minecraft assets.

Gameplay rules live in the create-engine crate (mixins/create/engine), plain Rust
with no game types, unit-tested with cargo test. runtime.rs is the thin game glue
injected as crate::game::create; hook helpers stay one call long. Generated files
(engine/src/generated.rs, content/) come from tools/create-data compile: never edit
them by hand. All AST changes go through horizon-mixins plan/apply; do not add
string-based Rust source rewriting. Never commit Create's src/main/resources/assets
or anything derived from it.

Validate with cargo fmt, clippy with warnings denied, cargo test and
python3 tools/test_template.py. For bundle/compatibility changes also run
python3 tools/test_stage.py and python3 tools/horizon.py check/test against the
pinned source export. Keep validation claims and supported revisions honest.
