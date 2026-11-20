//! Water wheels (`WaterWheelBlock`, `LargeWaterWheelBlock`, `WaterWheelBlockEntity`,
//! `WaterWheelStructuralBlock`): where the flow is, what it turns, the plank material, and the
//! facings and controller cells a player builds.
use crate::actions::{Action, Face};
use crate::scenario::{Change, Placed, Pos, Scenario};

fn stone(p: Pos) -> Placed {
    Placed {
        pos: p,
        state: "minecraft:stone".into(),
        motor: None,
    }
}
fn placed(p: Pos, state: &str) -> Placed {
    Placed {
        pos: p,
        state: state.into(),
        motor: None,
    }
}
fn use_at(tick: u32, pos: Pos, item: &str, face: Face, sneaking: bool) -> Change {
    Change {
        tick,
        pos,
        state: None,
        motor: None,
        turn: None,
        action: Some(Action::Use {
            item: item.into(),
            face,
            hit: face.point(0.5, 0.5),
            yaw: 0.,
            pitch: 0.,
            sneaking,
        }),
    }
}
fn use_look(tick: u32, pos: Pos, item: &str, face: Face, yaw: f32, pitch: f32) -> Change {
    let mut change = use_at(tick, pos, item, face, false);
    if let Some(Action::Use {
        yaw: y, pitch: p, ..
    }) = &mut change.action
    {
        *y = yaw;
        *p = pitch;
    }
    change
}
fn add(a: Pos, b: Pos) -> Pos {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn neg(a: Pos) -> Pos {
    [-a[0], -a[1], -a[2]]
}

/// A falling water column beside the wheel at `wheel`.
fn column(sc: &mut Scenario, wheel: Pos, q: Pos) {
    fluid_column(sc, wheel, q, "minecraft:water");
}

/// A falling column of `fluid` beside the wheel at `wheel`: a source over the falling cell `q`,
/// enclosed in stone so the native fluid never leaves the fixture. The wheel reads the falling
/// cell.
fn fluid_column(sc: &mut Scenario, wheel: Pos, q: Pos, fluid: &str) {
    let up = [0, 1, 0];
    let ring = [
        add(q, neg(up)),
        add(q, add(up, up)),
        add(q, [1, 0, 0]),
        add(q, [-1, 0, 0]),
        add(q, [0, 0, 1]),
        add(q, [0, 0, -1]),
        add(q, add(up, [1, 0, 0])),
        add(q, add(up, [-1, 0, 0])),
        add(q, add(up, [0, 0, 1])),
        add(q, add(up, [0, 0, -1])),
    ];
    for p in ring {
        if p != wheel {
            sc.blocks.push(stone(p));
        }
    }
    // The source first: a falling cell with nothing above it drains before the source arrives.
    sc.blocks
        .push(placed(add(q, up), &format!("{fluid}[level=0]")));
    sc.blocks.push(placed(q, &format!("{fluid}[level=8]")));
}

/// The two lateral sample directions of a wheel on `axis`.
fn normals(axis: &str) -> [[i32; 3]; 2] {
    match axis {
        "x" => [[0, 0, -1], [0, 0, 1]],
        _ => [[1, 0, 0], [-1, 0, 0]],
    }
}

/// The family's `k`th seed picks the variants.
pub fn generate(seed: u64, k: u64) -> Scenario {
    let axis = if k.is_multiple_of(2) { "x" } else { "z" };
    let facing = if axis == "x" { "east" } else { "south" };
    let sides = normals(axis);
    let mut sc = Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 0,
        checkpoints: vec![],
        blocks: vec![],
        events: vec![],
    };
    // A: a small wheel in a column of flowing water, its material used on, its flow stopped.
    let a: Pos = [-9, 8, 0];
    // The wheel's cell is stone while its streams settle, then the wheel is put in.
    sc.blocks.push(stone(a));
    sc.blocks.push(stone(add(a, [0, 1, 0])));
    column(&mut sc, a, add(a, sides[0]));
    if k % 4 < 2 {
        // A second column on the other side adds the opposite score.
        column(&mut sc, a, add(a, sides[1]));
    }
    if k & 8 != 0 {
        // A wheel built before its streams: each score change re-places its part.
        sc.blocks.insert(
            0,
            placed(a, &format!("create:water_wheel[facing={facing}]")),
        );
    } else {
        sc.blocks
            .push(placed(a, &format!("create:water_wheel[facing={facing}]")));
    }
    // B: player-placed small wheels: along the wheel's axis allowed, across it refused, and a
    // different axis beside it refused too.
    let b: Pos = [-3, 8, 0];
    sc.blocks.push(stone([b[0], b[1] - 1, b[2]]));
    let c = add(b, [0, 0, 1]);
    // C: a large wheel whose plane holds a stone (the item retries at the clicked face), then a
    // second wheel extending it.
    let d: Pos = [4, 8, 0];
    sc.blocks.push(stone([d[0], d[1] - 1, d[2]]));
    let wheel = [d[0], d[1] + 1, d[2]];
    sc.blocks.push(stone(add(wheel, [0, 2, 0])));
    // D: a large wheel blocked in a frame cell in the same tick its own tick runs.
    let e: Pos = [-9, 9, 6];
    sc.blocks.push(stone([e[0], e[1] - 2, e[2]]));
    // E: reload keeps the wheel's material and flow score.
    let f: Pos = [4, 8, 6];
    sc.blocks
        .push(placed(f, &format!("create:water_wheel[facing={facing}]")));
    sc.blocks.push(stone(add(f, [0, 1, 0])));
    // The other half reloads a *flowing* wheel: its saved network is restored by both sides.
    if k & 8 == 0 {
        column(&mut sc, f, add(f, sides[0]));
    }
    // F: a lava column turns a wheel the other way and reads the lava input.
    if k % 4 == 1 {
        let g: Pos = [-3, 8, 6];
        sc.blocks.push(placed(g, "create:water_wheel[facing=east]"));
        fluid_column(&mut sc, g, add(g, [0, 0, -1]), "minecraft:lava");
    }
    let n = sc.blocks.len() as u32;
    sc.ticks = n + 165;
    sc.checkpoints = vec![
        n + 1,
        n + 30,
        n + 45,
        n + 60,
        n + 80,
        n + 110,
        n + 139,
        n + 164,
    ];
    // A's interactions, on the block above the wheel so a non-consuming use cannot place.
    let materials = [
        "minecraft:oak_planks",
        "minecraft:spruce_planks",
        "create:andesite_casing",
        "create:wrench",
        "create:blaze_cake",
    ];
    for (i, item) in materials.iter().enumerate() {
        sc.events
            .push(use_at(n + 2 + i as u32, a, item, Face::Up, false));
    }
    if k % 8 >= 4 {
        let source = add(add(a, sides[0]), [0, 1, 0]);
        sc.events.push(Change::set(n + 34, source, "minecraft:air"));
    }
    // B: place, extend, and refuse.
    sc.events.push(use_at(
        n + 4,
        [b[0], b[1] - 1, b[2]],
        "create:water_wheel",
        Face::Up,
        false,
    ));
    sc.events
        .push(use_at(n + 10, b, "create:water_wheel", Face::South, false));
    sc.events
        .push(use_at(n + 16, c, "create:water_wheel", Face::East, false));
    let yaw = if k.is_multiple_of(4) { 90. } else { 0. };
    sc.events
        .push(use_look(n + 22, c, "create:water_wheel", Face::Up, yaw, 0.));
    // C: the retry placement, the extension, the frame's material, a wrench and a break.
    sc.events.push(use_at(
        n + 6,
        [d[0], d[1] - 1, d[2]],
        "create:large_water_wheel",
        Face::Up,
        false,
    ));
    sc.events.push(use_at(
        n + 26,
        wheel,
        "create:large_water_wheel",
        Face::South,
        false,
    ));
    let cell = add(wheel, [0, 1, 0]);
    sc.events.push(use_at(
        n + 32,
        cell,
        "minecraft:oak_planks",
        Face::Up,
        false,
    ));
    sc.events.push(use_at(
        n + 38,
        cell,
        "create:andesite_casing",
        Face::Up,
        false,
    ));
    sc.events.push(use_at(
        n + 44,
        add(wheel, [1, 0, 0]),
        "create:wrench",
        Face::Up,
        true,
    ));
    // The second wheel's own frame cell is broken later.
    sc.events.push(Change::set(
        n + 90,
        add(add(wheel, [0, 0, 1]), [-1, 0, 0]),
        "minecraft:air",
    ));
    // D: the wheel and the casing in its frame in one tick; its own tick breaks the wheel.
    if k % 4 != 3 {
        sc.events.push(use_at(
            n + 8,
            [e[0], e[1] - 2, e[2]],
            "create:large_water_wheel",
            Face::Up,
            false,
        ));
        sc.events.push(use_look(
            n + 8,
            add(e, [1, 0, 0]),
            "create:andesite_casing",
            Face::Down,
            0.,
            90.,
        ));
    }
    // E: the material and the reload.
    sc.events
        .push(use_at(n + 50, f, "minecraft:oak_planks", Face::Up, false));
    sc.events.push(Change {
        tick: n + 70,
        pos: f,
        state: None,
        motor: None,
        turn: None,
        action: Some(Action::Reload),
    });
    sc
}
