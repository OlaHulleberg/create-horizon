# Third-party notices

## Create (MIT code and data)

This port translates Create 6.0.11's Java code and compiles its data (everything outside
`src/main/resources/assets/`, `src/generated/` included) into Rust. Source:
<https://github.com/Creators-of-Create/Create>, branch `mc1.21.1/dev`, file `LICENSE.md`.

    MIT License

    Copyright (c) The Create Team / The Creators of Create

    Permission is hereby granted, free of charge, to any person obtaining a copy
    of this software and associated documentation files (the "Software"), to deal
    in the Software without restriction, including without limitation the rights
    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
    copies of the Software, and to permit persons to whom the Software is
    furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all
    copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
    SOFTWARE.

## Ponder and Catnip (MIT)

Create's utility library Catnip ships in its Ponder project; the port translates a few of its helpers
(`VecHelper.rotate` for the water wheel's flow score; `LerpedFloat`'s `Chaser.EXP` for the flywheel and
the train HUD; `AngleHelper.getShortestAngleDiff` for the train HUD's arrow), and the content mod carries
its placement indicator texture unchanged (`content/assets/create/textures/gui/placement_indicator.png`, under Create's namespace,
from `common/src/main/resources/assets/ponder/textures/gui/placement_indicator.png` at commit
`c3e5a41380203e1dd1e2431c494ec491a51965a5`; the repository's only license is this MIT one). Source:
<https://github.com/Creators-of-Create/Ponder>, branch `mc1.21.1/dev`, file `LICENSE`:
"MIT License, Copyright (c) 2022 The Create Team", with the same permission notice as above.

## NeoForge common tags (facts)

`tools/create-data/src/common_tags.rs` lists which vanilla items belong to 60 of NeoForge's common
(`c:`) item tags that Create's recipes use (`c:ingots/iron` is `minecraft:iron_ingot`), as NeoForge 21.1
for Minecraft 1.21.1 defines them (<https://github.com/neoforged/NeoForge>, branch `1.21.1`,
`src/generated/resources/data/c/tags/item`). The table is our own, written from those memberships; no
NeoForge file is copied.

## Create (assets: All Rights Reserved)

`src/main/resources/assets/` (textures, hand-written models, sounds, shaders, ponder) is
"All Rights Reserved, Copyright (c) The Create Team / The Creators of Create". None of it is in
this repository or in the mod's bundle: `tools/create-prepare` reads it from the player's own
Create jar on their machine.

## Horizon Veinminer

The build and staging tools (`tools/`) are adapted from the MIT-licensed Veinminer port; see
`LICENSE`.
