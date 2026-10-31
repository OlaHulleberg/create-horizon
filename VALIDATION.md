# Validation

Run on 2026-10-31 in the port sandbox (Linux, Rust 1.95.0 for this workspace, Horizon pinned at
`9d979caa48a3bb58203319b9ea044db58c38e0d8`, horizon-mixins `d9c83d7`).

| Check | Result |
|---|---|
| `cargo fmt --all --check` | Pass |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | Pass |
| `cargo test --workspace --locked --offline` | Engine and create-data unit tests pass |
| `python3 tools/test_template.py` | The real bundle (named hook, glue module, engine crate) compiles on a two-crate fixture; its hook test runs the glue and the original body; AST audit, unchanged bytes/mtimes, wrong revision and renamed hook target rejected |
| `python3 tools/test_stage.py --horizon /workspace/horizon` | The full pinned export stages deterministically; checkout unchanged; only the planned HEAD and module differ |
| `python3 tools/horizon.py stage/check/test/build --horizon /workspace/horizon` | Pass; `test` also runs the engine's tests as a member of the staged Horizon workspace |

In game (headless, `/workspace/headless.sh`, renderer `Intel(R) Graphics (BMG G21)`, Vulkan): with the prepared
content mod the game registers 25,349 Create states from id 35,723 and the mod logs its range; without it the mod
logs that it stays idle. Create's decoration blocks place and render with Create's textures. Performance: see
PORTING.md, "Budgets".
