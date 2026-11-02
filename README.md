# Create for Horizon

A port of [Create](https://github.com/Creators-of-Create/Create) 6.0.11 to
[Horizon](https://github.com/horizon-rust/horizon) (Minecraft 26.3 in Rust) as a pure-Rust
**horizon-mixins** native mod. No Java runs, at build time or play time. The plan, what is done, and
the measured budgets are in [PORTING.md](PORTING.md).

It builds against Horizon's `feat/create-hooks` branch on
[OlaHulleberg/horizon](https://github.com/OlaHulleberg/horizon/tree/feat/create-hooks): Horizon 9d979ca plus the
native hook points Create needs (API 41 to 66), pinned in `compatibility.json`. Create's own textures, models and
sounds (`src/main/resources/assets`, All Rights Reserved) are not here: `tools/create-data prepare` reads them from a
Create jar you have.

## Layout

```text
compatibility.json                 Pinned Horizon / horizon-mixins revisions and source hashes
mixins/create/
  horizon-mixin.toml               Named hooks, the glue module and the engine crate
  runtime.rs                       Game glue, injected as crate::game::create (thin: hooks -> engine)
  *.rs                             One small typed helper per hook
  engine/                          create-engine: Create's rules as plain Rust, unit-tested here,
                                   staged as a member of Horizon's workspace
    src/generated.rs               @generated block table (tools/create-data compile)
content/                           @generated content-mod declarations (tools/create-data compile)
tools/create-data/                 compile: Create's MIT data -> tables; prepare: Create jar -> content mod
tools/horizon.py                   Export -> plan -> apply -> check/build/test against the pin
tools/test_template.py             Compiles the real bundle on a tiny fixture and runs its hook test
tools/test_stage.py                Real pinned source audit and repeatability test
tools/perf_headless.sh             Horizon's perf_bench.py, invisibly (Xvfb, --offscreen, scratch homes)
tools/audit/                       Rust AST comparison against the pristine export
```

## Licences

Create's code and data (everything outside its `src/main/resources/assets/`) are MIT and are read,
translated and compiled here; its assets are All Rights Reserved and never committed or shipped:
`create-data prepare` reads them from the player's own Create jar. See [LICENSE](LICENSE) and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Build and run

Requirements: Linux, Rust 1.95.0 (the template workspace) and the Rust of the Horizon pin, Python
3.12+, a Horizon Git clone holding the pinned commit and horizon-mixins at its pinned revision
(found beside this repository or beside `Create/`, or `HORIZON_MIXINS`).

```sh
cargo run -p create-data -- compile --create .. --out .           # after a Create data change
cargo run -p create-data -- prepare --out target/create-content /path/to/create-6.0.11.jar
python3 tools/horizon.py build --horizon /path/to/horizon          # always pass --horizon
target/horizon-cargo/fast/horizon-client --resources /path/to/prepared/26.3 \
    --dev-mod $PWD/target/create-content
```

In this repository's sandbox the jar stand-in is two folders:
`prepare --out target/create-content ../src/generated/resources ../src/main/resources`.

## Validation

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
python3 tools/test_template.py
python3 tools/horizon.py stage|check|test|build --horizon /path/to/horizon
```

`horizon.py test` runs the glue's tests inside Horizon's game test binary and the engine's tests as
a member of the staged Horizon workspace (Horizon's `Cargo.lock` is the allow list). See
[VALIDATION.md](VALIDATION.md).

Build and staging tools are adapted from the MIT-licensed Horizon native mod template and the
Veinminer port.
