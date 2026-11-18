# Measured comparison results

Measured on 2026-11-19, macOS arm64, Rust 1.99.1, JDK 25.0.2, Python 3.9.6. Probe: default checked `dev` profile (`debug=0`, incremental compilation off).

Subject: `adf6d88295df9be2a261ba3a5002e96de8820a2c`; engine tree `30febfbf9ec26c58ea90151c3027a2d4d5c9e13e`. No engine changes. Java: Create 6.0.11 source at `37ea4afcfe5bf85875d4b0749798a45330070a1f`. This is a method-fixture comparison, **not an in-game test**.

| Suite | Cases | Match | Difference | Unavailable | Engine panic |
|---|---:|---:|---:|---:|---:|
| motion-primitives | 35192 | 35192 | 0 | 0 | 0 |
| belt-surface | 1600 | 1088 | 512 | 0 | 0 |
| fan-entity | 12192 | 216 | 0 | 11976 | 0 |
| transport-processing | 1448 | 1448 | 0 | 0 | 0 |
| transport | 65256 | 64598 | 0 | 658 | 0 |
| burner | 17710 | 17030 | 0 | 384 | 296 |
| network | 75 | 75 | 0 | 0 | 0 |
| processing | 9282 | 9154 | 0 | 128 | 0 |
| placement | 27648 | 27648 | 0 | 0 | 0 |
| rotation | 31752 | 31752 | 0 | 0 | 0 |
| milling | 29593 | 29525 | 68 | 0 | 0 |
| drilling | 39970 | 23585 | 0 | 16385 | 0 |
| visual | 6388 | 6205 | 0 | 183 | 0 |
| fan | 17680 | 17672 | 8 | 0 | 0 |
| fan-media | 27648 | 6912 | 0 | 20736 | 0 |
| basin | 7147 | 4803 | 24 | 2320 | 0 |
| **Total** | 330581 | 276903 | 612 | 52770 | 296 |

All 16 suites compiled and ran. No harness error. The full comparison is **not a complete match**. Unavailable cases are not differences or passes. These are counts of input scenarios; many contain multiple recorded ticks. They are not coverage percentages.

## Differences to review

- **Belt surface phase (512 cases):** fallback Java emits a half-cycle offset for the single diagonal surface; the public engine surface/scroll observation differs. Example: input `0 1 0 -256 0 0` gives `diagonal_start 1 0.1875` in Java, `diagonal_start 1 0` in Rust. This does not compare the Flywheel shader, GPU output or full game renderer.
- **Basin application (24 cases):** success and/or inventory state differs for the preserved source contexts, including combined input/output consumption, repeated fluid requirements and output overbooking. Some source behaviours are surprising; these are observations to inspect, not an automatic recommendation to copy every Java quirk.
- **Fan restored timer states (8 cases):** for dropped-item timers restored as −3/−2 with the same processing type, Java continues decrementing while the public engine rejects further processing. Example: `D 0 -3 0,17,1,4`. These are adversarial restored states, not a claim of failure in an ordinary fresh recipe.
- **Seeded output rolls (68 cases):** Create uses Java randomness and the engine documents xorshift64*. Example: `R 0 1 0.5` gives count 0 in Java, 1 in Rust. Same-seed sequence equality is therefore not expected; this is recorded separately from a recipe/timer bug. Draw counts are not compared.
- **Burner checked arithmetic (296 cases):** adversarial fuel values near the i32 boundary panic in the checked Rust build, while Java wraps. Example: `I 1 1 0 1 1 2147483647 0 0` overflows `remaining + time`. This is profile-dependent and must not be presented as a demonstrated release-build or ordinary-game crash.

No mechanics were changed to make these comparisons pass. The JSON report records bounded reproductions per nonpassing category and the unchanged Java method/source hashes.

## Limits and validation

- The five gating suites (`network`, `placement`, `rotation`, `transport-processing`, `motion-primitives`) match in every case. Other suites are observational in CI.
- 14 comparison-harness/provenance unit tests pass. Input hashes for 14 transferred matrices were checked independently against their original generators at the pinned origin commit.
- Probe formatting and `cargo clippy --all-targets -- -D warnings` pass.
- Existing `create-engine` tests: **776 passed**. These are the project’s existing tests, not 776 newly added comparisons.
- No game, player world, UI, sound backend, mod package, signed SDK or full mod build was exercised for this test-only PR. The existing mod CI is unchanged.
- The [migration ledger](MIGRATION.md) inventories all 66 original reference-oriented programs. 15 contribute a matrix or observation here; 51 still need a suitable paired-world, SDK, client or private-reference adapter. The supplemental motion primitive suite does not replace those live tests.

Reproduce with the commands in [README.md](README.md). Generated full reports belong in `target/java-parity/` or CI artifacts rather than source control. These numbers are a dated snapshot, not an allowlist of differences and not a baseline that suppresses future comparisons.
