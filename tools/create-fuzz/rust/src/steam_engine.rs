//! Steam engines and the boilers that drive them (`SteamEngineBlock`, `SteamEngineBlockEntity`,
//! `PoweredShaftBlock`, `PoweredShaftBlockEntity`, `BoilerData`, `BoilerHeaters`): where an
//! engine attaches, the shaft it turns, the boiler's heat and water supply, and what a player
//! does to them.
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
fn add(a: Pos, b: Pos) -> Pos {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// A tank of `width` x `width` x `height` blocks with its corner at `corner`.
fn tank(sc: &mut Scenario, corner: Pos, width: i32, height: i32) {
    for x in 0..width {
        for y in 0..height {
            for z in 0..width {
                sc.blocks.push(placed(
                    add(corner, [x, y, z]),
                    "create:fluid_tank[bottom=true,shape=window,top=true]",
                ));
            }
        }
    }
}

/// The family's `k`th seed picks the variants.
pub fn generate(seed: u64, k: u64) -> Scenario {
    let big = k % 4 < 2;
    let cake = k % 8 < 6;
    let mut sc = Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 0,
        checkpoints: vec![],
        blocks: vec![],
        events: vec![],
    };
    // A: a tank with heat under it and an engine on each of three faces.
    let width = if big { 3 } else { 2 };
    let a = [0, 5, 0];
    tank(&mut sc, a, width, 2);
    if cake {
        sc.blocks.push(placed(
            [0, 4, 0],
            "create:blaze_burner[blaze=smouldering,facing=north]",
        ));
    } else if k % 16 < 10 {
        // Passive heat (`create:passive_boiler_heaters`): magma under the tank.
        sc.blocks.push(placed([0, 4, 0], "minecraft:magma_block"));
    } else {
        // A campfire heats while it is lit (`BlockHelper.isNotUnheated`).
        let lit = if k.is_multiple_of(2) { "true" } else { "false" };
        sc.blocks.push(placed(
            [0, 4, 0],
            &format!(
                "minecraft:campfire[facing=north,lit={lit},signal_fire=false,waterlogged=false]"
            ),
        ));
    }
    let floor_engine = [0, 7, 0];
    let floor_shaft = [0, 9, 0];
    let east = [width, 5, 0];
    let east_shaft = [width + 2, 5, 0];
    let north = [0, 5, -1];
    let north_shaft = [0, 5, -3];
    // The shafts an engine converts when it is placed (`ShaftBlock.pickCorrectShaftType`).
    sc.blocks.push(placed(
        floor_shaft,
        "create:shaft[axis=x,waterlogged=false]",
    ));
    // A shaft on the engine's own axis is not valid: it stays a plain shaft.
    sc.blocks
        .push(placed(east_shaft, "create:shaft[axis=x,waterlogged=false]"));
    sc.blocks.push(placed(
        north_shaft,
        "create:shaft[axis=x,waterlogged=false]",
    ));
    sc.blocks.push(placed(
        floor_engine,
        "create:steam_engine[face=floor,facing=north,waterlogged=false]",
    ));
    sc.blocks.push(placed(
        east,
        "create:steam_engine[face=wall,facing=east,waterlogged=false]",
    ));
    sc.blocks.push(placed(
        north,
        "create:steam_engine[face=wall,facing=north,waterlogged=false]",
    ));
    // A waterlogged engine must be boxed in: its source spreads into open neighbours. The
    // stones go after it so its shape change runs while it holds water.
    let wet = [0, 5, width];
    sc.blocks.push(placed(
        wet,
        "create:steam_engine[face=wall,facing=south,waterlogged=true]",
    ));
    for o in [
        [0, 1, 0],
        [0, -1, 0],
        [-1, 0, 0],
        [1, 0, 0],
        [0, 0, 1],
        [0, 0, 2],
    ] {
        sc.blocks.push(stone(add(wet, o)));
    }
    // B: a second tank with two passive heaters and two engines; its shaft is put in by the
    // engine's placement helper, and the boiler is given no water.
    let b = [7, 5, 0];
    tank(&mut sc, b, 2, 2);
    sc.blocks.push(placed([7, 4, 0], "minecraft:magma_block"));
    sc.blocks.push(placed([8, 4, 1], "minecraft:magma_block"));
    sc.blocks
        .push(placed([11, 5, 0], "create:shaft[axis=z,waterlogged=false]"));
    sc.blocks.push(placed(
        [9, 5, 0],
        "create:steam_engine[face=wall,facing=east,waterlogged=false]",
    ));
    sc.blocks.push(placed(
        [7, 7, 0],
        "create:steam_engine[face=floor,facing=north,waterlogged=false]",
    ));
    // A waterlogged engine must be boxed in: its source spreads into open neighbours. The
    // stones go after it so its shape change runs while it holds water.
    let wet = [7, 5, 2];
    sc.blocks.push(placed(
        wet,
        "create:steam_engine[face=wall,facing=south,waterlogged=true]",
    ));
    for o in [
        [0, 1, 0],
        [0, -1, 0],
        [-1, 0, 0],
        [1, 0, 0],
        [0, 0, 1],
        [0, 0, 2],
    ] {
        sc.blocks.push(stone(add(wet, o)));
    }
    // C: an engine with no tank behind it.
    let c = [13, 5, 0];
    sc.blocks.push(stone(c));
    let n = sc.blocks.len() as u32;
    // The player feeds the burner, fills the boiler and places engines.
    sc.events.push(use_at(
        n + 2,
        [0, 4, 0],
        "create:blaze_cake",
        Face::Up,
        false,
    ));
    sc.events.push(use_at(
        n + 3,
        [0, 4, 0],
        "create:blaze_cake",
        Face::Up,
        false,
    ));
    sc.events.push(Change {
        tick: n + 4,
        pos: a,
        state: None,
        motor: None,
        turn: None,
        action: Some(Action::GameMode { creative: true }),
    });
    // Water while the boiler is active, then again after it sampled its supply. Tank B's top
    // corner, the third bucket's target, is gone when the boiler loses a tank block (below).
    let tank_block_removed = k % 8 == 3;
    for (i, tick) in [n + 6, n + 44, n + 74].into_iter().enumerate() {
        if i == 2 && tank_block_removed {
            continue;
        }
        let (corner, w) = if i == 2 { (b, 2) } else { (a, width) };
        sc.events.push(use_at(
            tick,
            [corner[0] + w - 1, corner[1] + 1, corner[2] + w - 1],
            "minecraft:water_bucket",
            Face::Up,
            false,
        ));
    }
    // An engine over stone has no tank to attach to.
    sc.events
        .push(use_at(n + 10, c, "create:steam_engine", Face::Up, false));
    // A floor engine placed on tank B's top, and the shaft through its helper (which places a
    // powered shaft), then a shaft through that powered shaft's own helper.
    sc.events.push(use_at(
        n + 14,
        [8, 6, 1],
        "create:steam_engine",
        Face::Up,
        false,
    ));
    sc.events.push(use_at(
        n + 18,
        [7, 7, 0],
        "create:shaft",
        Face::North,
        false,
    ));
    sc.events
        .push(use_at(n + 22, [7, 9, 0], "create:shaft", Face::Up, false));
    // A sneak use of the powered shaft removes it (`IWrenchable.onSneakWrenched`).
    sc.events
        .push(use_at(n + 26, [7, 9, 0], "create:wrench", Face::Up, true));
    // A wrench turned against its tank cannot stand.
    sc.events
        .push(use_at(n + 28, north, "create:wrench", Face::Up, false));
    // An engine removed, or a tank block taken out of the boiler.
    if k % 4 == 1 {
        sc.events.push(Change {
            tick: n + 32,
            pos: [9, 5, 0],
            state: None,
            motor: None,
            turn: None,
            action: Some(Action::Remove),
        });
    }
    if tank_block_removed {
        sc.events.push(Change {
            tick: n + 36,
            pos: [8, 6, 1],
            state: Some("minecraft:air".into()),
            motor: None,
            turn: None,
            action: None,
        });
    }
    // Half the seeds reload a running boiler: its engines' saved size is restored by both sides.
    if k & 16 == 0 {
        sc.events.push(Change {
            tick: n + 60,
            pos: floor_engine,
            state: None,
            motor: None,
            turn: None,
            action: Some(Action::Reload),
        });
    }
    sc.ticks = n + 160;
    sc.checkpoints = vec![
        n + 1,
        n + 12,
        n + 24,
        n + 34,
        n + 48,
        n + 66,
        n + 100,
        n + 139,
        n + 159,
    ];
    sc
}
