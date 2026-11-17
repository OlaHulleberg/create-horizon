//! Metal girders and girder encased shafts (`GirderBlock`, `GirderEncasedShaftBlock`): what they
//! read of the blocks beside them, their shape after a neighbour changed, their placement, and a
//! shaft used on a girder.

use super::super::server::ServerLevel;
use super::super::states;
use super::{add, block_of, face_of, flag, prop, with, with_props};
use create_engine::Block;
use create_engine::decor::{self, Side};
use create_engine::kinetics::{Axis, Dir};
use create_engine::parts::StateInfo;
use mod_api::{BlockShapeKind, BlockState, SupportType};

/// What a girder at `pos` reads of the block on side `d` (`GirderBlock.updateState`,
/// `updateVerticalProperty`, `isConnected`).
pub(super) fn side(lv: &ServerLevel<'_>, pos: [i32; 3], d: Dir) -> Side {
    let p = add(pos, d);
    let Some(state) = lv.block_state(p) else {
        return Side::default();
    };
    let host = &*lv.host;
    let text = super::state_text(host, state);
    let (name, props) = states::split(&text);
    let get = |k: &str| props.iter().find(|(a, _)| *a == k).map(|(_, v)| *v);
    let empty = lv
        .level
        .block_shape(host, state, p, BlockShapeKind::Collision)
        .is_none_or(|s| s.is_empty());
    let bracket = with(lv.dim, |x| x.brackets.get(&p).copied()).is_some_and(|b| b.facing == d);
    let placard_toward = name == "create:placard"
        && get("facing")
            .and_then(super::dir_named)
            .map(|f| decor::connected_direction(get("face").unwrap_or(""), f))
            == Some(d);
    let holds = d.axis() == Axis::Y
        && (get("up") == Some("true")
            || name == "create:display_board"
            || (name.ends_with("lantern")
                && get("hanging").is_some_and(|h| (h == "true") == (d == Dir::Down)))
            || (name.ends_with("chain") && get("axis") == Some("y"))
            || (get("face") == Some("ceiling") && d == Dir::Down)
            || (get("face") == Some("floor") && d == Dir::Up));
    Side {
        air: matches!(
            name,
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        ),
        girder: (name == "create:metal_girder")
            .then(|| (get("x") == Some("true"), get("z") == Some("true"))),
        encased: (name == "create:metal_girder_encased_shaft").then(|| {
            if get("axis") == Some("x") {
                Axis::X
            } else {
                Axis::Z
            }
        }),
        toward: bracket || placard_toward,
        holds,
        full_face: lv
            .level
            .block_face_sturdy(host, p, face_of(d.opposite()), SupportType::Full)
            == Some(true),
        empty,
        chute_down: (name == "create:chute" && get("facing") == Some("down"))
            || name == "create:smart_chute",
    }
}

/// A girdable track (`create:track`) above beside `pos` across `d`'s axis, running along it
/// (`TrackShape.XO` / `ZO`).
pub(super) fn track_beside(lv: &ServerLevel<'_>, pos: [i32; 3], d: Dir) -> bool {
    let axis = d.axis();
    if axis == Axis::Y {
        return false;
    }
    let across = if axis == Axis::X { Axis::Z } else { Axis::X };
    let want = if axis == Axis::X { "xo" } else { "zo" };
    [true, false].into_iter().any(|positive| {
        let p = add(
            [pos[0], pos[1] + 1, pos[2]],
            Dir::from_axis(across, positive),
        );
        lv.block_state(p).is_some_and(|s| {
            s.block().id(&*lv.host) == "create:track"
                && prop(&*lv.host, s, "shape").as_deref() == Some(want)
        })
    })
}

fn girder_of(info: &StateInfo, props: &dyn Fn(&str) -> bool) -> decor::Girder {
    decor::Girder {
        axis: info.axis.unwrap_or(Axis::Y),
        x: props("x"),
        z: props("z"),
        top: props("top"),
        bottom: props("bottom"),
    }
}

fn axis_name(a: Axis) -> &'static str {
    match a {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

/// The girder at `pos` takes `g`'s properties.
pub(super) fn set_girder(lv: &mut ServerLevel<'_>, pos: [i32; 3], g: decor::Girder) {
    set_girder_with(lv, pos, g, super::super::server::update_all());
}

/// The girder at `pos` takes `g`'s properties, set with `flags`.
fn set_girder_with(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    g: decor::Girder,
    flags: mod_api::BlockUpdateFlags,
) {
    let Some(state) = lv.block_state(pos) else {
        return;
    };
    let next = with_props(
        &*lv.host,
        state,
        &[
            ("axis", axis_name(g.axis)),
            ("x", flag(g.x)),
            ("z", flag(g.z)),
            ("top", flag(g.top)),
            ("bottom", flag(g.bottom)),
        ],
    );
    if let Some(next) = next
        && next != state
    {
        lv.set_state(pos, next, flags);
    }
}

/// `GirderBlock.getStateForPlacement`, after the girder is placed: its axis from the clicked
/// face, then every side.
pub(super) fn place(lv: &mut ServerLevel<'_>, pos: [i32; 3], clicked: Dir) {
    let g = decor::placement(clicked, |d| side(lv, pos, d), |d| track_beside(lv, pos, d));
    set_girder(lv, pos, g);
}

/// A girder or encased shaft shapes itself again after its neighbours changed: every side, as
/// `updateShape` for each (the hook doesn't say which neighbour it was).
pub(super) fn reshape(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some(state) = lv.block_state(pos) else {
        return;
    };
    let Some((block, info)) = block_of(lv.block(pos)) else {
        return;
    };
    let host = &*lv.host;
    let text = super::state_text(host, state);
    let (_, own) = states::split(&text);
    let props = |k: &str| own.iter().any(|(a, v)| *a == k && *v == "true");
    let support_empty = |d: Dir| {
        let x = side(lv, pos, d);
        x.air || x.empty
    };
    match block {
        Block::MetalGirder => {
            let mut g = girder_of(&info, &props);
            for d in Dir::ALL {
                g = decor::update_shape(
                    g,
                    d,
                    |d| side(lv, pos, d),
                    support_empty,
                    |d| track_beside(lv, pos, d),
                );
            }
            set_girder(lv, pos, g);
        }
        Block::MetalGirderEncasedShaft => {
            let was = (props("top"), props("bottom"));
            let mut tb = was;
            for d in [Dir::Up, Dir::Down] {
                tb = decor::encased_vertical(tb, d, &side(lv, pos, d), support_empty(d));
            }
            if tb != was
                && let Some(next) =
                    with_props(host, state, &[("top", flag(tb.0)), ("bottom", flag(tb.1))])
            {
                lv.put(pos, next);
            }
        }
        _ => {}
    }
}

/// The girders and encased shafts beside `pos` shape themselves again (a bracket came or went:
/// `updateNeighbourShapes`).
pub(super) fn around(dim: usize, pos: [i32; 3]) {
    with(dim, |d| {
        for side in Dir::ALL {
            d.girder_checks.push(add(pos, side));
        }
    });
}

/// `GirderBlock.useItemOn` with a shaft: the girder becomes an encased shaft (its top, bottom
/// and water kept), with the netherite block's hit sound.
pub(super) fn encase(lv: &mut ServerLevel<'_>, pos: [i32; 3], state: BlockState, clicked: Dir) {
    let host = &*lv.host;
    let get = |k: &str| prop(host, state, k).unwrap_or_else(|| "false".into());
    let axis = decor::encased_axis(get("x") == "true", clicked);
    let Some(next) = super::state_with(
        host,
        Block::MetalGirderEncasedShaft,
        &[
            ("axis", axis_name(axis)),
            ("top", &get("top")),
            ("bottom", &get("bottom")),
            ("waterlogged", &get("waterlogged")),
        ],
    ) else {
        return;
    };
    crate::glue::items::wrench::switch_state(lv, pos, next);
    lv.block_sound("minecraft:block.netherite_block.hit", pos, 0.5, 1.25);
}

/// `GirderWrenchBehavior.handleClick`: the wrench toggles the girder's top or bottom bracket on
/// the side nearest the hit, and the girder beyond's opposite one when they pair
/// (`setBlock(.., 2 | 16)`: no neighbour updates).
pub(super) fn wrench(lv: &mut ServerLevel<'_>, pos: [i32; 3], point: [f64; 3]) {
    let Some(state) = lv.block_state(pos) else {
        return;
    };
    let Some((_, info)) = block_of(lv.block(pos)) else {
        return;
    };
    let host = &*lv.host;
    let text = super::state_text(host, state);
    let (_, own) = states::split(&text);
    let props = |k: &str| own.iter().any(|(a, v)| *a == k && *v == "true");
    let g = girder_of(&info, &props);
    let hit = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]) - 0.5);
    let other = |d: Dir| side(lv, pos, d).girder;
    let Some((d, pair)) = decor::girder_wrench(&g, hit, other) else {
        return;
    };
    let mut g = g;
    if d == Dir::Up {
        g.top = !g.top;
    } else {
        g.bottom = !g.bottom;
    }
    // `setBlock(.., 2 | 16)`: its clients told, no neighbour updated.
    let flags = mod_api::BlockUpdateFlags::CLIENTS | mod_api::BlockUpdateFlags::KNOWN_SHAPE;
    set_girder_with(lv, pos, decor::girder_post_process(g), flags);
    let beyond = add(pos, d);
    if pair && let Some((Block::MetalGirder, info)) = block_of(lv.block(beyond)) {
        let Some(state) = lv.block_state(beyond) else {
            return;
        };
        let text = super::state_text(&*lv.host, state);
        let (_, own) = states::split(&text);
        let props = |k: &str| own.iter().any(|(a, v)| *a == k && *v == "true");
        let mut o = girder_of(&info, &props);
        if d == Dir::Up {
            o.bottom = !o.bottom;
        } else {
            o.top = !o.top;
        }
        set_girder_with(lv, beyond, decor::girder_post_process(o), flags);
    }
}

/// `GirderEncasedShaftBlock.onWrenched`: the girder it was (`getRotatedBlockState`: beams across
/// the shaft, its top, bottom and water kept), the turning sound, and the shaft back to a
/// player outside creative.
pub(super) fn unencase(lv: &mut ServerLevel<'_>, player: mod_api::PlayerRef, pos: [i32; 3]) {
    let Some(state) = lv.block_state(pos) else {
        return;
    };
    let host = &*lv.host;
    let get = |k: &str| prop(host, state, k).unwrap_or_else(|| "false".into());
    let along_x = get("axis") == "x";
    let Some(next) = super::state_with(
        host,
        Block::MetalGirder,
        &[
            ("x", flag(!along_x)),
            ("z", flag(along_x)),
            ("axis", if along_x { "z" } else { "x" }),
            ("top", &get("top")),
            ("bottom", &get("bottom")),
            ("waterlogged", &get("waterlogged")),
        ],
    ) else {
        return;
    };
    lv.put(pos, next);
    crate::glue::items::wrench::rotate_sound(lv, pos);
    if !super::creative(&*lv.host, player)
        && let Some(shaft) = super::stack_of(&*lv.host, "create:shaft", 1)
    {
        crate::glue::items::give_back(lv.host, player, shaft);
    }
}

/// `GirderPlacementHelper.canExtendToward`: a line of girders goes on toward `side` through a
/// girder along that way (both ways for a cross, up and down for a post) and a girder encased
/// shaft across its shaft, never up or down.
fn can_extend_toward(lv: &ServerLevel<'_>, pos: [i32; 3], side: Dir) -> bool {
    let Some((block, info)) = block_of(lv.block(pos)) else {
        return false;
    };
    let axis = side.axis();
    match block {
        Block::MetalGirder => {
            let Some(state) = lv.block_state(pos) else {
                return false;
            };
            let host = &*lv.host;
            let x = prop(host, state, "x").as_deref() == Some("true");
            let z = prop(host, state, "z").as_deref() == Some("true");
            match (x, z) {
                (false, false) => axis == Axis::Y,
                (true, true) => true,
                _ => axis == if x { Axis::X } else { Axis::Z },
            }
        }
        Block::MetalGirderEncasedShaft => axis != Axis::Y && Some(axis) != info.axis,
        _ => false,
    }
}

/// `GirderPlacementHelper.getOffset` and `PlacementOffset.placeInWorld`: a girder item on a
/// girder goes past the girders in line with it on the side nearest the hit (within
/// `placementAssistRange`, 4 more with an extendo grip), along that way, shaped by its
/// neighbours (`updateFromNeighbourShapes`). None: no place (the click goes on as usual).
pub(super) fn helper(
    lv: &mut ServerLevel<'_>,
    player: mod_api::PlayerRef,
    click: &mod_api::BlockClick<'_>,
    stack: &mod_api::ItemStack,
) -> Option<mod_api::InteractionResult> {
    let (at, d) = target(lv, player, click.pos, click.hit.point)?;
    {
        let water = lv
            .block_state(at)
            .and_then(|s| lv.state_name(s))
            .is_some_and(|n| n == "minecraft:water");
        let axis = d.axis();
        let state = super::state_with(
            &*lv.host,
            Block::MetalGirder,
            &[
                ("axis", axis_name(axis)),
                ("x", flag(axis == Axis::X)),
                ("z", flag(axis == Axis::Z)),
                ("waterlogged", flag(water)),
            ],
        )?;
        lv.set_state(at, state, super::super::server::update_all());
        reshape(lv, at);
        lv.place_sound(at);
        super::use_one(&mut *lv.host, player, click.hand, stack);
        Some(mod_api::InteractionResult::Success)
    }
}

/// `GirderPlacementHelper.getOffset`: where a girder item clicked at `point` on the girder at
/// `pos` goes, and the way it extends.
fn target(
    lv: &ServerLevel<'_>,
    player: mod_api::PlayerRef,
    pos: [i32; 3],
    point: [f64; 3],
) -> Option<([i32; 3], Dir)> {
    let hit = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]) - 0.5);
    let mut range = create_engine::place::PLACEMENT_ASSIST_RANGE;
    if super::ladders::gripping(lv, player) {
        range += 4;
    }
    let dirs = create_engine::place::ordered_by_distance(hit, |d| can_extend_toward(lv, pos, d));
    for d in dirs {
        let mut poles = 0;
        let mut at = add(pos, d);
        while can_extend_toward(lv, at, d) {
            poles += 1;
            at = add(at, d);
        }
        if poles >= range || !lv.free(at) {
            continue;
        }
        return Some((at, d));
    }
    None
}

/// The girder helper's target for the indicator: a girder item on a girder, and the state it
/// takes there (`withAxis`, then `updateFromNeighbourShapes`), for its ghost.
pub(crate) fn helper_target(
    lv: &ServerLevel<'_>,
    player: mod_api::PlayerRef,
    pos: [i32; 3],
    point: [f64; 3],
    item: &str,
) -> Option<crate::glue::interaction::HelperTarget> {
    if item != "create:metal_girder"
        || block_of(lv.block(pos)).map(|b| b.0) != Some(Block::MetalGirder)
    {
        return None;
    }
    let (at, d) = target(lv, player, pos, point)?;
    let axis = d.axis();
    let mut g = decor::Girder {
        axis,
        x: axis == Axis::X,
        z: axis == Axis::Z,
        top: false,
        bottom: false,
    };
    for d in Dir::ALL {
        g = decor::update_shape(
            g,
            d,
            |d| side(lv, at, d),
            |d| {
                let x = side(lv, at, d);
                x.air || x.empty
            },
            |d| track_beside(lv, at, d),
        );
    }
    let props = [
        ("axis", axis_name(g.axis)),
        ("x", flag(g.x)),
        ("z", flag(g.z)),
        ("top", flag(g.top)),
        ("bottom", flag(g.bottom)),
    ];
    Some((at, props.map(|(k, v)| (k, v.to_owned())).to_vec()))
}
