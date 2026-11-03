<!-- Draft for arismoko/horizon#27 (design note before code, as arismoko asked on #44). Not posted.
     Re-check against the fork's latest main before posting; update the "reflects main at" line. -->

🤖 Agent: OlaHulleberg's AI agent (Claude Code, Claude Opus 5.5), not OlaHulleberg personally.

As suggested on #44, here is a design note for **moving block structures** (horizon-rust/horizon#5313) before any
code. It reflects main at `479d364c4`, and what we learnt porting Create's contraptions (bearings, pistons, pulleys,
gantries, minecart contraptions, trains) on an older Horizon.

**The short version.** Of what #5313 lists (collision, saving, sync, rendering), three already have a home here or one
on the way: rendering through instanced drawing on `DrawContext` (the #26 Rendering row), saving through attachments,
sync through mod messages. The piece with no home is **collision with moving blocks**. We propose to start there,
with a small level service, and to leave the carrier (entity, block entity or level list) to the mod for now.

**Why not an entity type first.** A mod entity type is a template copy (only `minecraft:snowball` today), and
#5313's first question (entity or block entity) blocks everything behind it. In our port the carrier never needed to
be an entity: a contraption was the mod's own typed data, anchored to its controller (a bearing's position, a cart's
entity id, a train's id), saved with the anchor and sent to clients as mod messages. What it couldn't do without
Horizon was let players and mobs stand on it, be pushed by it, and ride along.

**Proposal: moving colliders.** A mod gives a level a set of colliders, each a moving frame and its boxes:

- `LevelRef::set_collider(host, id, frame, boxes)` and `remove_collider(host, id)`, `id` the mod's own key (namespaced
  by the registrar's mod id, as parts are).
- `frame`: an origin, a rotation (a quaternion, so bearings and turntables work, not translation only) and the frame's
  motion this tick (linear velocity and angular velocity about the origin).
- `boxes`: block-sized or smaller boxes in the frame (a block's collision shape, `getCollisionShape`), set when the
  structure changes, not every tick. Only the frame changes per tick.

What the game does with them, in the shared motion paths (`motion.rs`, `collisions.rs`) for players, mobs and item
entities, server and client alike:
1. **Solid.** Entity motion collides with the boxes at the frame's pose this tick, as with blocks (oriented boxes
   tested in the frame's space).
2. **Carried.** An entity standing on a box (on the ground against it last tick) moves with the frame: the frame's
   motion at the entity's feet, rotation included, before its own motion, and its yaw turns with the frame.
3. **Pushed.** A frame moving into an entity pushes it out along the move, not just the nearest face.

None of this calls into the mod during motion: the game reads the frame and boxes the mod set, so it stays within
world work's rules (no `Host` inside simulation).

**What it doesn't cover, deliberately.** Rendering (instancing), saving (attachments), sync (mod messages: clients get
frames from the mod's own messages and set their copy, and prediction uses the same paths), blocks acting while moving
(Create's actors: drills, harvesters; the mod's own tick), clicking a moving block, and seats (riding needs a vehicle;
a later step, maybe the first use of a mod entity type that isn't a template copy).

**What we measured on the old build.** Our version was a hook the mod answered each tick with boxes near an entity,
and a carry by the frame's move. It held 16 windmills, 16 minecart contraptions and 12 trains of 60 blocks at under
0.25 ms of tick. The lesson: collision must use the structure's real shape, rotated, and carry by the actual moved
delta, or players fall off turning structures. Asking the mod per entity per tick was the wrong shape: a level-owned
collider list the game reads is cheaper and needs no hook.

**Questions.**
1. Is a level service (`LevelRef` colliders) the right home, or should colliders hang off an attachment on the anchor?
2. Oriented boxes in a frame, or should a collider be a list of block positions plus states (the game derives shapes)?
3. Client prediction: should clients get colliders only through the mod's own messages, or should the game sync them?
4. Should vehicles and seats on a structure be part of this, or a separate later step?

If the direction looks right, we'd take it through #27, collisions first, with a throwaway trait mod as proof
(a rotating platform a player stands on, integrated and dedicated).
