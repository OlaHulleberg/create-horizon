# Sunrise nightly pin, 2026-11-18

Target: `2d8126336e579777eabf9b2fd8b3d476236a04f2` (Nightly 2026-11-17).
Mixins remains `e59da9094f0c2fdda6c627fe9eff22dec6a906f0`, matching the SDK metadata.

The SDK and `cargo-sunrise-mod` are official outputs of
`sdk-2d8126336e579777eabf9b2fd8b3d476236a04f2`. The tool was downloaded as
`tool-x86_64-unknown-linux-gnu.tar.xz` (SHA-256 matches the SDK metadata), not
built locally. The tool fetched and verified the SDK using its built-in published
release keys, without base or trusted-key overrides. The official launcher 1.0.1
and the official nightly Linux game were downloaded from Sunrise's releases.
Rust remains 1.99.1.

## Migration

Reviewed the vocabulary changes between b77574e and 2d81263. Applied changes:

| Changelog id / change | Migration |
|---|---|
| Sunrise rename | Manifest filename and native pin field, API crate/path, mod and part macros, tooling, CI and documentation renamed. Generator emits `data/create/sunrise/`; prepared behaviour overrides use `sunrise:behaviour`. Regenerated with `create-data compile --create /root/Create --out .`; generated engine table unchanged. |
| attachment-declarations | Declare all five existing persistent attachment identities at registration, without synchronization. |
| block-placed-level | Placement listeners use the event's level instead of deriving it from the placer; player-dependent operations retain their guards. |
| hit-sounds-entity-random | Incoming-damage dispatch and subsystem implementations accept HitHost. |
| random-source-trait | Remove obsolete trait imports; take the random trait object, use its bounded integer answer, express the old triangular draws from two float draws. |
| fall-inputs | Seat passes half the fall distance to the ordinary damage calculation through an engine helper. |
| item-model-layers | Item layers wrap the existing names; special renderer declares Stack policy and accepts the context argument. |
| menu-slot-icon | Existing slot positions become ScreenSlot values with no empty icon. |
| Screen layout | Explicit toolbox Screen sets the added big-result flag false. Other screens retain their defaults. |
| quick-move-empty / menu-hooks-decided-actions | Quick move reads an optional input and writes an optional remainder; only the event's container and remainder are edited. The previous whole-stack routing restriction is retained for a separate parity follow-up. |
| level-play-sound-except-player | Pass None to level sounds, preserving their audience. Player-local sounds retain their existing signature. |
| start-riding-refusal | Existing boolean decisions use is_ok(). |
| wire-roles-one-file-per-trait and hook/layout changes | Rebuild with the nightly SDK and macros. No handwritten wire data or hook versions. |

`remove-block-replacement` has no listener/call site here. Recipe serializers already
register both their types and raw serializers (`recipe-serializers-registered`).
Other changed hooks retain their supplied inputs or already filter their own entities;
new fields and additive APIs are not adopted. No game implementation or SDK code is
copied into the repository.

## Checks

All VALIDATION.md Checks commands pass on Rust 1.99.1 with `sunrise-mod` replacing
`horizon-mod`: create fmt, Clippy, 91 glue tests, 774 engine tests, SDK check;
engine fmt and Clippy; generator fmt, locked Clippy and 49 locked tests including
committed_assets with the cached Create jar. create-fuzz's release tests pass:
93 tests with CREATE_HORIZON_DIR set to this worktree.

Logs and downloaded tools are under `/mnt/lanes/agent-pin`. The isolated harness is
`/root/hz/hzn.sh`, instance create-nightly, display :89, under /mnt/lanes/hz-nightly.
The existing hz.sh/env.sh, create-main and :87 were not changed.

## Game smoke

The official launcher built the mod against the prebuilt nightly. Launcher Play
requires an account absent from the isolated launcher home; the harness instead
starts that built nightly client directly, with its prepared resources outside the
launcher's lease-managed cache and the prepared Create folder as --dev-mod.
Resources were prepared from the existing Minecraft install at
/root/.local/share/mc-26.3. Create's cached official jar was copied into the isolated
runtime jar cache. No game files are committed.

Fresh creative world `pin-smoke` loaded. Disabled pause on focus loss with F3+P.
Placed a motor at (0,100,0) facing east and shaft at (1,100,0): the shaft's
KineticState Speed reads 16, with a two-member network.
A three-segment horizontal belt at x=1..3,y=100,z=4, driven by a south-facing motor,
carried a summoned diamond to the depot at (0,100,4); Items reads one diamond.
A pump at (0,100,10), driven through an x-axis cog at (0,100,11) by a west-facing
motor at (1,100,11), moved water from the source pool west into the tank east.
Tank amount increased from 1000 through 1240 to 4008. A valve at (6,100,10),
facing up with axis_along_first=true, driven from its side, reached Pointer Value 1
and enabled=true. The earlier wrong motor orientations were corrected during setup.
Before save, ticks were frozen; saved the world and quit. Reloaded the world:
the shaft still reads 16, the depot still holds one diamond, the valve remains
Pointer Value 1 and enabled=true. The tank kept water and continued filling to
8000; saved and quit again. A second reload reads exactly 8000, then quit.

## Separate follow-ups

The APIs already catalogued in API_GAPS.md remain separate parity work:

- `shapes_from_block_entity` + `set_shapes`
- Scoreboard host calls
- `light_emission` / `friction` of a state
- `is_redstone_conductor`, `is_solid_render`, correct-tool queries
- `level_block_break_effect`
- `level_update_from_neighbour_shapes`
- `on_block_placed_by` v2 (level)
- `FluidInteraction` declarations
- Signal reads on `LevelHost` (fixes #7)
- `LevelHost::collision_shape`
- `level_place_block` without a player
- `entities_in` returns frames and paintings
- `level_portal_destination`
- `menu_quick_move` v3 (empty result)
- `set_collider`, `MoveCause::Carried`
- `full_attack_charge`
- `fall_on` v2 distance
- `set_sprinting`
- Client-side `brightness`
- `client_pick`
- `item_uses_block_light`
- `tick_rate_is_frozen`
- `play_sound` with `except`
- `SoundRef::move_to` / `set_pitch`
- Fix: one bad recipe no longer empties its type

The fall input and sound signature migrations above use only what the new signatures require.
