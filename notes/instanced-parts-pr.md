# Plan: the renderer half of horizon-rust/horizon#6302 (instanced moving parts)

Status: not started, nothing claimed. We said on #6302 (2026-11-02) that we would open the renderer half once the
maintainers choose option B or C and the `RenderHooks` question is settled; ErikKalikas reviews it and takes the hook
on top. Checked against arismoko/horizon `d62e8fdbb` (2026-11-03): `RenderHooks` exists in the generated mod API, so
that question looks settled; the B/C choice is still open and the fork's Rendering milestone (#44) is "Not started".
Re-check both, and which repo takes it, before starting (skill `horizon-pr`).

Source: our own `gfx/parts.rs` + `vs_part` (a0fccb2df) and helpers (a81b8ef53) on OlaHulleberg/horizon
`feat/create-hooks`, MIT. Rewrite on the target's latest main; don't port the branch.

## Scope of the PR (step 1 of #6302)

`Transformed` instances only (Rotating is the next PR, on top):
- a mesh per model: block-state models now (`block_model_vertices`), standalone models (#6360) as one more source;
- an instance = model, a 4×3 transform relative to its block, light sampled at its position;
- one instanced draw per model per frame, in the entity pass with the entity bind group.

## ErikKalikas's five points, and the answer to each

1. **Culling.** Instances kept in ranges per chunk section. Each frame, frustum-test the sections (the same test the
   terrain uses) and the 64-block block-entity distance (`getViewDistance`), and draw only the ranges in view
   (`multi_draw_indexed_indirect`, or one draw per visible range where that isn't there).
2. **One change re-uploads a whole model's list.** Dirty ranges per section: a block entity's instances are written
   in place with `queue.write_buffer` of just that range; a section's range grows by reallocating that section only.
3. **Lifetimes.** Instances are owned by the block entity's position and rebuilt only when its `revision()` moves
   (#6260, the `KeptGeometry` pattern in `game/native_block_entity_draw.rs`). Cleared on `finish_close_world`,
   `leave_server` and the client chunk unload seam (fork #283); meshes rebuilt on a resource reload
   (`apply_staged_pack`), since their UVs point into the old atlas. A test for each lifetime.
4. **Drawn straight into the pass.** State in the docs that shader packs, shadows, the occlusion cull and translucent
   sorting don't see instances yet (#6302 step 5).
5. **The clock** (matters for Rotating, fix it in the shared uniform now): pass game time as whole ticks modulo a
   period that divides every `rpm × 60` used, plus the partial tick, as two values, so the angle never jumps at the
   wrap and keeps f32 precision.

## The API (the hook is ErikKalikas's PR; ours must leave room for it)

A block-entity part method, client side, called when the block entity's revision moves (never per frame), that fills
an instance list: `fn block_entity_instances(&self, host: &dyn Host, event: &mut BlockEntityInstancesEvent<'_>)`.
No NeoForge event matches (Flywheel's `BlockEntityVisualizer`), so it keeps a Horizon name. Fragment with
`side = "client"` and a `mod_api` contract with `scope = "block_entity"`.

## Proof

- Unit tests: section-range bookkeeping (insert, change in place, remove, section move), lifetimes.
- A throwaway trait mod: 64 × 63 = 4,096 instances, frame p50 against the same scene drawn through
  `block_entity_submit` and against static vanilla blocks; tick unchanged. Spread over many chunks too, for culling.
- Integrated server and a dedicated server's client (only what the update tag carries).

## Then (separate PRs)

`Rotating` (axis, rpm, phase; angle on the GPU), then a scrolling instance for belts (Create needs both).
