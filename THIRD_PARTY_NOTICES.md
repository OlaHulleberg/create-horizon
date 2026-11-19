# Third-party notices

This repository is MIT licensed ([LICENSE](LICENSE)). Create's All Rights Reserved assets are excluded (see below).
No NeoForge (LGPL) source or data, no horizon-rust/horizon code at or after `fb1135c6` (LGPL), and no code or tables
of Sunrise's own are used.

## Create (MIT code and data)

This port translates Create's Java code (the 6.0.11 source, branch `mc1.21.1/dev`) and compiles its data
(everything outside `src/main/resources/assets/`, `src/generated/` included) into Rust; the jar the player's game
reads Create's assets from is the 6.0.10 release. The committed 26.3 registry overlays are derived from that
release's MIT `src/generated` data at `ac0c444d9828da3453ae8cc65338e8de063286fb`; the simple item definitions
refer to model IDs verified against the same tree, not copies of the models. Source:
<https://github.com/Creators-of-Create/Create>, file `LICENSE.md`.

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

A few helpers of Catnip, Create's utility library in its Ponder project, are translated (`VecHelper.rotate`,
`LerpedFloat`'s `Chaser.EXP`, `AngleHelper.getShortestAngleDiff`). Its placement indicator texture is committed
unchanged as `create/assets/create/textures/gui/placement_indicator.png` (from
`common/src/main/resources/assets/ponder/textures/gui/placement_indicator.png` at commit
`c3e5a41380203e1dd1e2431c494ec491a51965a5`). Source: <https://github.com/Creators-of-Create/Ponder>, branch
`mc1.21.1/dev`, file `LICENSE`: "MIT License, Copyright (c) 2022 The Create Team", with the permission notice above.

## Create (assets: All Rights Reserved)

`src/main/resources/assets/` (textures, models, sounds, shaders, ponder) is "All Rights Reserved, Copyright (c)
The Create Team / The Creators of Create". None of it is in this repository or shipped from it: the player's
machine downloads Create's official jar from Modrinth (checked by its pinned SHA-512), and the launcher and
`tools/create-data prepare` read it there.

## arismoko/horizon (MIT)

The mod is written against the native mod API that began in the MIT-licensed arismoko/horizon fork
(<https://github.com/arismoko/horizon>), which Sunrise carries on. Vanilla facts the port tabulates were looked up in
it: the effects' colours and the potions' effects (`create/src/engine/src/potion.rs`, from its
`crates/horizon-client-core/src/potions.rs`), the dye colours of fireworks (`create/src/glue/items/filters.rs`, from
`crates/horizon-items/src/firework.rs`) and the instant effects' amounts (`create/src/engine/src/open_end.rs`).
"MIT License, Copyright (c) 2026 Ludovic Marie and the Horizon contributors", with the permission notice above.
