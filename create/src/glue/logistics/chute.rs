//! Chutes, smart chutes and item vaults on the server: stacks falling through chutes into what
//! lies below (`ChuteBlockEntity.tick`), or blown up them by an encased fan, empty chutes taking
//! from what is above (`handleInputFromAbove`) or below while blown up, a smart chute's filter
//! slot, and vaults' shapes.

use super::super::server::{ServerLevel, UPDATE_CLIENTS};
use super::super::world::with_world;
use super::items;
use super::{info_at, mark_touched};
use crate::glue::contraptions::actors;
use crate::glue::items::filters;
use create_engine::Block;
use create_engine::kinetics::Dir;
use mod_api::{BlockClick, Direction, HostBlockStates, ItemStack, Tag};

/// The chute or smart chute at `pos`, with whether it is powered.
pub fn chute_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(Block, bool)> {
    let (b, i) = info_at(lv, pos)?;
    matches!(b, Block::Chute | Block::SmartChute).then_some((b, i.powered))
}

/// `ChuteBlockEntity.canAcceptItem` for the chute or smart chute `block` at `pos` (its state
/// `powered`): a smart chute also not while powered and only what its filter passes
/// (`SmartChuteBlockEntity.canAcceptItem`). Emptiness is the caller's.
pub fn chute_accepts(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    block: Block,
    powered: bool,
    stack: &ItemStack,
) -> bool {
    let dim = lv.dim;
    match block {
        Block::Chute => true,
        Block::SmartChute => {
            !powered
                && with_world(|w| {
                    filters::passes(&*lv.host, w.dims[dim].chute_filters.get(&pos), stack)
                })
                .unwrap_or(false)
        }
        _ => false,
    }
}

pub fn chute_stack(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<ItemStack> {
    items::slot(lv, pos, 0)
}

pub fn set_chute_stack(lv: &mut ServerLevel<'_>, pos: [i32; 3], stack: Option<ItemStack>) {
    items::set_slot(lv, pos, 0, stack);
    // Arms using the chute look again.
    mark_touched(lv.dim, pos);
    crate::glue::advancements::award_placer(lv, pos, "chute");
}

/// `ChuteBlockEntity` taking an item that landed on it (`canAcceptItem`: only while empty; a
/// smart chute's filter and power): the whole stack, at the top. True when it took it.
pub fn chute_take(lv: &mut ServerLevel<'_>, pos: [i32; 3], stack: &ItemStack) -> bool {
    let Some((block, powered)) = chute_at(lv, pos) else {
        return false;
    };
    if !chute_accepts(lv, pos, block, powered, stack) || chute_stack(lv, pos).is_some() {
        return false;
    }
    if !items::set_slot(lv, pos, 0, Some(stack.clone())) {
        return false;
    }
    let dim = lv.dim;
    with_world(|w| w.dims[dim].chutes.insert(pos, 1.0));
    crate::glue::advancements::award_placer(lv, pos, "chute");
    true
}

/// The way the chute at `pos` leads (`getChuteFacing`): down, or a diagonal chute's facing; None
/// when no chute is there.
pub fn chute_facing(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Dir> {
    let (b, i) = info_at(lv, pos)?;
    match b {
        Block::Chute => Some(i.facing.unwrap_or(Dir::Down)),
        Block::SmartChute => Some(Dir::Down),
        _ => None,
    }
}

/// A player's click on a chute (`AbstractChuteBlock.useItemOn`, `ChuteBlock.useItemOn`,
/// `ChuteItem.place`): an empty hand takes what it holds; an industrial iron block encases a
/// chute (not used up); a chute item on a chute's side places a diagonal chute leading into it,
/// a block up (or turns the down-facing chute there toward it). True when the click was the
/// chute's.
pub fn use_chute(lv: &mut ServerLevel<'_>, pos: [i32; 3], click: &BlockClick<'_>) -> bool {
    let Some(p) = lv.host.player(click.player) else {
        return false;
    };
    let held = click.stack.map(|s| items::id_of(&*lv.host, s));
    let Some((block, _)) = info_at(lv, pos) else {
        return false;
    };
    let shape = || {
        lv.block_state(pos).and_then(|s| {
            s.property(&lv.host.block_states(), "shape")
                .ok()
                .flatten()
                .and_then(create_engine::chute::Shape::read)
        })
    };
    match held.as_deref() {
        None => {
            if click.hand != mod_api::Hand::Main {
                return false;
            }
            let Some(stack) = chute_stack(lv, pos) else {
                return false;
            };
            set_chute_stack(lv, pos, None);
            let dim = lv.dim;
            with_world(|w| w.dims[dim].chutes.remove(&pos));
            items::give(&mut *lv.host, click.player, stack);
            true
        }
        Some("create:industrial_iron_block") if block == Block::Chute => {
            use create_engine::chute::Shape;
            if matches!(shape(), Some(Shape::Intersection | Shape::Encased) | None) {
                return false;
            }
            lv.set_property(pos, "shape", "encased", super::super::server::update_all());
            let at = pos.map(|v| f64::from(v) + 0.5);
            lv.level.play_sound(
                &mut *lv.host,
                "minecraft:block.netherite_block.hit",
                at,
                mod_api::SoundSource::Blocks,
                0.5,
                1.05,
            );
            true
        }
        Some("create:chute") => {
            let sneaking = p.is_secondary_use_active(&*lv.host).unwrap_or(false);
            let Some(face) = Dir::from_offset(click.hit.face) else {
                return false;
            };
            if sneaking || face.axis() == create_engine::kinetics::Axis::Y {
                return false;
            }
            let o = face.offset();
            let correct = [pos[0] + o[0], pos[1] + 1, pos[2] + o[2]];
            place_diagonal(lv, correct, face, p, click)
        }
        _ => false,
    }
}

/// `ChuteItem.place` at `at`, facing `face`: a new chute where the block can be replaced, else a
/// down-facing chute there turned toward `face`.
fn place_diagonal(
    lv: &mut ServerLevel<'_>,
    at: [i32; 3],
    face: Dir,
    player: mod_api::PlayerRef,
    click: &BlockClick<'_>,
) -> bool {
    use create_engine::chute::{Shape, update_state};
    let existing = info_at(lv, at);
    let free = lv
        .block_state(at)
        .is_none_or(|s| lv.host.block_states().replaceable(s) != Some(false));
    let shape = match existing {
        Some((Block::Chute, i)) if i.facing.unwrap_or(Dir::Down) == Dir::Down => lv
            .block_state(at)
            .and_then(|s| {
                s.property(&lv.host.block_states(), "shape")
                    .ok()
                    .flatten()
                    .and_then(Shape::read)
            })
            .unwrap_or(Shape::Normal),
        _ if free => Shape::Normal,
        _ => return true,
    };
    let above = [at[0], at[1] + 1, at[2]];
    let funnel_down = super::funnel::funnel_at(lv, above)
        .is_some_and(|f| f.belt.is_none() && f.facing == Dir::Down);
    let (f, s) = update_state(at, face, shape, |p| chute_facing(lv, p), funnel_down);
    let state = {
        let facts = lv.host.block_states();
        mod_api::BlockState::of(
            &facts,
            super::super::states::key(Block::Chute),
            &[
                mod_api::StateConstraint::new("facing", super::super::kinetics::dir_name(f)),
                mod_api::StateConstraint::new("shape", s.name()),
            ],
        )
    };
    let Ok(state) = state else {
        return false;
    };
    lv.set_state(at, state, super::super::server::update_all());
    if free {
        lv.place_sound(at);
        if !crate::glue::items::creative(&*lv.host, player) {
            crate::glue::items::shrink_held(&mut *lv.host, player, click.hand, 1);
        }
    }
    true
}

/// Chutes whose shape is due a look (`AbstractChuteBlock.tick` and `updateShape` from above,
/// `updateChuteState`): a diagonal chute with nothing to lead into turns down; intersections
/// come and go with the diagonal chutes leading in.
pub fn tick_chute_shapes(lv: &mut ServerLevel<'_>) {
    use create_engine::chute::{Shape, update_state};
    let dim = lv.dim;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].chute_shapes)).unwrap_or_default();
    if checks.is_empty() {
        return;
    }
    checks.sort();
    checks.dedup();
    for pos in checks {
        let Some((Block::Chute, info)) = info_at(lv, pos) else {
            continue;
        };
        let Some(state) = lv.block_state(pos) else {
            continue;
        };
        let shape = {
            let facts = lv.host.block_states();
            state
                .property(&facts, "shape")
                .ok()
                .flatten()
                .and_then(Shape::read)
                .unwrap_or(Shape::Normal)
        };
        let facing = info.facing.unwrap_or(Dir::Down);
        let above = [pos[0], pos[1] + 1, pos[2]];
        let funnel_down = super::funnel::funnel_at(lv, above)
            .is_some_and(|f| f.belt.is_none() && f.facing == Dir::Down);
        let (f2, s2) = update_state(pos, facing, shape, |p| chute_facing(lv, p), funnel_down);
        if (f2, s2) == (facing, shape) {
            continue;
        }
        let next = {
            let facts = lv.host.block_states();
            state
                .with(&facts, "facing", super::super::kinetics::dir_name(f2))
                .and_then(|s| s.with(&facts, "shape", s2.name()))
        };
        if let Ok(next) = next {
            lv.set_state(pos, next, super::super::server::update_all());
        }
    }
}

/// Whether the block at `pos` lets a chute's stack out toward it: below, unless it can hold
/// a rigid block up on its top (`Block.canSupportRigidBlock`); above, unless its bottom is a
/// full solid side (`BlockHelper.hasBlockSolidSide(.., DOWN)`).
fn falls_through(lv: &ServerLevel<'_>, pos: [i32; 3], up: bool) -> bool {
    let (face, support) = if up {
        (mod_api::BlockFace::Down, mod_api::SupportType::Full)
    } else {
        (mod_api::BlockFace::Up, mod_api::SupportType::Rigid)
    };
    lv.level.block_face_sturdy(&*lv.host, pos, face, support) != Some(true)
}

/// The speed of an encased fan at `pos` blowing `way` (`fan.getSpeed()`), if one is there.
fn fan_blowing(lv: &ServerLevel<'_>, pos: [i32; 3], way: Dir) -> Option<f32> {
    let (block, info) = info_at(lv, pos)?;
    if block != Block::EncasedFan || info.facing != Some(way) {
        return None;
    }
    let dim = lv.dim;
    with_world(|w| w.speed(dim, pos)).flatten()
}

/// `getItemMotion` of the chute at `pos`: its column's `push` (`calculatePush`: a fan under the
/// lowest chute blowing up) and `pull` (`calculatePull`: a fan over the highest one blowing
/// down), in chutes a tick.
pub fn chute_motion(lv: &ServerLevel<'_>, pos: [i32; 3]) -> f32 {
    create_engine::chute::item_motion(chute_push(lv, pos, 0), chute_pull(lv, pos, 0))
}

/// `getInputChutes`: the chutes leading into the one at `pos`, the one above facing down and
/// the diagonal ones above beside it facing toward it.
fn input_chutes(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Vec<[i32; 3]> {
    [Dir::Down, Dir::North, Dir::South, Dir::West, Dir::East]
        .into_iter()
        .filter_map(|f| {
            let o = if f == Dir::Down { [0; 3] } else { f.offset() };
            let p = [pos[0] + o[0], pos[1] + 1, pos[2] + o[2]];
            (chute_facing(lv, p) == Some(f)).then_some(p)
        })
        .collect()
}

/// `calculatePull`: a fan over the chute at `pos` blowing down, else what the chutes leading
/// into it pull, added up (`depth`: how far up it looked).
fn chute_pull(lv: &ServerLevel<'_>, pos: [i32; 3], depth: u32) -> f32 {
    if let Some(speed) = fan_blowing(lv, [pos[0], pos[1] + 1, pos[2]], Dir::Down) {
        return speed;
    }
    if depth > 256 {
        return 0.0;
    }
    input_chutes(lv, pos)
        .into_iter()
        .map(|p| chute_pull(lv, p, depth + 1))
        .sum()
}

/// `calculatePush`: a fan under the chute at `pos` blowing up, else the push of the chute it
/// leads into (diagonals too), shared among that chute's inputs.
fn chute_push(lv: &ServerLevel<'_>, pos: [i32; 3], depth: u32) -> f32 {
    if let Some(speed) = fan_blowing(lv, [pos[0], pos[1] - 1, pos[2]], Dir::Up) {
        return speed;
    }
    let Some(facing) = chute_facing(lv, pos) else {
        return 0.0;
    };
    let target = create_engine::chute::target(pos, facing);
    if depth > 256 || chute_at(lv, target).is_none() {
        return 0.0;
    }
    let branches = input_chutes(lv, target).len().max(1) as f32;
    chute_push(lv, target, depth + 1) / branches
}

/// A stack the fans carry up the chute at `pos` (`tick` with a positive motion,
/// `handleUpwardOutput`): into the container above, else the chute above if it is empty and
/// does not fall (`getInputChutes`, `canAcceptItem`), else out of the top when nothing solid is
/// over it; it waits in the middle while the way is shut. The chute's next position, None when
/// the stack left.
fn rise(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    position: f32,
    motion: f32,
    stack: ItemStack,
    active: bool,
) -> Option<f32> {
    use create_engine::chute::{Fall, step};
    let dim = lv.dim;
    let above = [pos[0], pos[1] + 1, pos[2]];
    let above_chute = chute_at(lv, above);
    let container = above_chute.is_none()
        && (items::items_at(lv, above, Some(Direction::Down)).is_some()
            || actors::is_item_interface(lv, above));
    let can = if !active {
        false
    } else if let Some((b, powered)) = above_chute {
        chute_stack(lv, above).is_none()
            && chute_accepts(lv, above, b, powered, &stack)
            && chute_motion(lv, above) >= 0.0
    } else if container {
        true
    } else {
        let funnel_up = super::funnel::funnel_at(lv, above).is_some_and(|f| f.facing == Dir::Up);
        !funnel_up && falls_through(lv, above, true)
    };
    match step(position, motion, can) {
        Fall::At(p) => Some(p),
        Fall::Out if above_chute.is_some() => {
            set_chute_stack(lv, above, Some(stack));
            set_chute_stack(lv, pos, None);
            with_world(|w| w.dims[dim].chutes.insert(above, 0.0));
            None
        }
        Fall::Out if container => {
            let count = stack.count();
            let left = items::insert_item(lv, above, Direction::Down, stack);
            if left.as_ref().is_some_and(|l| l.count() == count) {
                Some(0.5)
            } else {
                let done = left.is_none();
                set_chute_stack(lv, pos, left);
                (!done).then_some(0.5)
            }
        }
        Fall::Out => {
            // Out of the top, half a block over the centre, at twice the chute's speed.
            let at = [
                f64::from(pos[0]) + 0.5,
                f64::from(pos[1]) + 1.0,
                f64::from(pos[2]) + 0.5,
            ];
            set_chute_stack(lv, pos, None);
            items::drop_stack(lv, at, [0.0, f64::from(motion) * 2.0, 0.0], stack, 0);
            None
        }
    }
}

/// Chutes holding a stack (`ChuteBlockEntity.tick` falling, `handleDownwardOutput`): into the
/// container below (`insertItemStacked`; a vault's whole inventory), else the chute below if it
/// is empty (and a smart chute's filter passes it), else out of the bottom when nothing solid is
/// under it; it waits in the middle while the way is shut, or while a smart chute is powered
/// (`canActivate`). Then the empty chutes that may take from the inventory above them do.
pub fn tick_chutes(lv: &mut ServerLevel<'_>) {
    use create_engine::chute::{Fall, step};
    let dim = lv.dim;
    let moving: Vec<([i32; 3], f32)> =
        with_world(|w| w.dims[dim].chutes.iter().map(|(p, v)| (*p, *v)).collect())
            .unwrap_or_default();
    // The chutes whose stack moved, and those next to them it may have moved into: their
    // windows show it again (`show_chute_item`).
    let mut shown: Vec<[i32; 3]> = Vec::new();
    for (pos, _) in &moving {
        shown.push(*pos);
        shown.push([pos[0], pos[1] - 1, pos[2]]);
        shown.push([pos[0], pos[1] + 1, pos[2]]);
    }
    for (pos, position) in moving {
        let Some(stack) = chute_stack(lv, pos) else {
            with_world(|w| {
                w.dims[dim].chutes.remove(&pos);
                // Emptied (a hopper below took it): it may take from above again.
                w.dims[dim].chute_pulls.wake(pos);
            });
            continue;
        };
        let active = !matches!(chute_at(lv, pos), Some((_, true)));
        if !active && position <= 0.5 {
            // A powered smart chute holds its stack in the middle; it ticks again once its
            // `powered` changes.
            with_world(|w| w.dims[dim].chutes.remove(&pos));
            continue;
        }
        let motion = chute_motion(lv, pos);
        if motion > 0.0 {
            let next = rise(lv, pos, position, motion, stack, active);
            with_world(|w| match next {
                Some(p) => {
                    w.dims[dim].chutes.insert(pos, p);
                }
                None => {
                    w.dims[dim].chutes.remove(&pos);
                    w.dims[dim].chute_pulls.wake(pos);
                }
            });
            continue;
        }
        // `handleDownwardOutput`: the inventory below first; then the chute it leads into (a
        // diagonal chute's beside that); a diagonal chute never drops what it holds.
        let facing = chute_facing(lv, pos).unwrap_or(Dir::Down);
        let diagonal = facing != Dir::Down;
        let below = [pos[0], pos[1] - 1, pos[2]];
        let target = create_engine::chute::target(pos, facing);
        let target_chute = chute_at(lv, target);
        // A belt below takes it through its segment's handler (`ItemHandlerBeltSegment`).
        let belt_below = chute_at(lv, below).is_none()
            && super::belt::belt_segment(lv, below).is_some_and(|(_, _, s)| s.transports());
        let container = chute_at(lv, below).is_none()
            && (belt_below
                || items::items_at(lv, below, Some(Direction::Up)).is_some()
                || actors::is_item_interface(lv, below));
        let target_takes = |lv: &ServerLevel<'_>| {
            target_chute.is_some_and(|(b, powered)| {
                chute_stack(lv, target).is_none() && chute_accepts(lv, target, b, powered, &stack)
            })
        };
        let can = if !active {
            false
        } else if container {
            true
        } else if target_chute.is_some() {
            target_takes(lv)
        } else if diagonal {
            false
        } else {
            let funnel_down = super::funnel::funnel_at(lv, below)
                .is_some_and(|f| f.belt.is_none() && f.facing == Dir::Down);
            !funnel_down && falls_through(lv, below, false)
        };
        // Into the chute it leads into: at the top, or past the middle from a diagonal one.
        let into_target = |lv: &mut ServerLevel<'_>, stack: ItemStack| {
            set_chute_stack(lv, target, Some(stack));
            set_chute_stack(lv, pos, None);
            let at = if diagonal { 0.51 } else { 1.0 };
            with_world(|w| w.dims[dim].chutes.insert(target, at));
        };
        let next = match step(position, motion, can) {
            Fall::At(p) => Some(p),
            Fall::Out if container => {
                let count = stack.count();
                let left = if belt_below {
                    // The whole stack (to its largest size) as one belt item, at the segment's
                    // middle, if the segment has room.
                    let (id, progress) = super::belt::belt_name(&*lv.host, &stack);
                    let n = count.min(u32::from(u8::MAX)) as u8;
                    let left = super::belt::put_on_from_above(lv, below, &id, n, progress, false)
                        .map_or(count, |l| count - u32::from(n - l));
                    if left == 0 {
                        None
                    } else {
                        items::with_count(&stack, left)
                    }
                } else {
                    items::insert_item(lv, below, Direction::Up, stack.clone())
                };
                if left.as_ref().is_some_and(|l| l.count() == count) {
                    if diagonal && target_takes(lv) {
                        into_target(lv, stack);
                        None
                    } else {
                        Some(0.5)
                    }
                } else {
                    let done = left.is_none();
                    set_chute_stack(lv, pos, left);
                    (!done).then_some(0.5)
                }
            }
            Fall::Out if target_chute.is_some() => {
                into_target(lv, stack);
                None
            }
            Fall::Out => {
                // An item 12/16 below the centre, falling at a quarter block a tick.
                let at = [
                    f64::from(pos[0]) + 0.5,
                    f64::from(pos[1]) + 0.5 - 0.75,
                    f64::from(pos[2]) + 0.5,
                ];
                set_chute_stack(lv, pos, None);
                items::drop_stack(lv, at, [0.0, -0.25, 0.0], stack, 0);
                None
            }
        };
        with_world(|w| match next {
            Some(p) => {
                w.dims[dim].chutes.insert(pos, p);
            }
            None => {
                w.dims[dim].chutes.remove(&pos);
                w.dims[dim].chute_pulls.wake(pos);
            }
        });
    }
    tick_chute_pulls(lv);
    shown.sort();
    shown.dedup();
    for pos in shown {
        show_chute_item(lv, pos);
    }
}

/// Block entity keys of a windowed chute's stack, for its clients to draw it moving
/// (`ChuteRenderer`): the item (an empty string: none), where it is (1 the top, 0 the bottom)
/// and how far it moves a tick.
pub const CHUTE_ITEM: &str = "ChuteItem";
pub const CHUTE_POSITION: &str = "ChutePosition";
pub const CHUTE_MOTION: &str = "ChuteMotion";

/// Tells the clients of the vertical windowed chute at `pos` what it holds and where
/// (`CHUTE_ITEM`), when that changed.
fn show_chute_item(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    if chute_facing(lv, pos) != Some(Dir::Down) {
        return;
    }
    let window = lv.block_state(pos).is_some_and(|s| {
        s.property(&lv.host.block_states(), "shape").ok().flatten() == Some("window")
    });
    if !window {
        return;
    }
    let dim = lv.dim;
    let stack = chute_stack(lv, pos);
    let id = stack
        .as_ref()
        .map(|s| items::key_of(&*lv.host, s))
        .unwrap_or_default();
    let moving = with_world(|w| w.dims[dim].chutes.get(&pos).copied()).flatten();
    let position = moving.unwrap_or(0.5);
    let motion = if moving.is_some() && stack.is_some() {
        chute_motion(lv, pos)
    } else {
        0.0
    };
    let Some(e) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let data = e.data(&*lv.host).unwrap_or_default();
    let same = matches!(data.get(&CHUTE_ITEM.into()), Some(Tag::String(s)) if s.to_string_lossy() == id)
        && matches!(data.get(&CHUTE_POSITION.into()), Some(Tag::Float(b)) if f32::from_bits(*b) == position)
        && matches!(data.get(&CHUTE_MOTION.into()), Some(Tag::Float(b)) if f32::from_bits(*b) == motion);
    if same {
        return;
    }
    let _ = e.merge_data(
        &mut *lv.host,
        mod_api::Compound::from([
            (CHUTE_ITEM.into(), Tag::String(id.as_str().into())),
            (CHUTE_POSITION.into(), Tag::Float(position.to_bits())),
            (CHUTE_MOTION.into(), Tag::Float(motion.to_bits())),
        ]),
    );
}

/// `ChuteBlockEntity.handleInputFromAbove` for the empty chutes due to look: up to 16 of the
/// first item of the inventory above (a smart chute: up to 64 of what its filter passes, and
/// nothing while powered) start falling from the top. One that finds nothing sleeps until a
/// container of Create's says it changed (`invVersionTracker`), or looks again `RETRY` ticks
/// later under another container; with no container above it waits for a neighbour change.
fn tick_chute_pulls(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let time = lv.game_time();
    let due = with_world(|w| w.dims[dim].chute_pulls.take_due(time)).unwrap_or_default();
    for pos in due {
        let Some((block, powered)) = chute_at(lv, pos) else {
            continue;
        };
        if powered || chute_stack(lv, pos).is_some() {
            // Powered: it looks again when that changes; holding a stack: when it is empty.
            continue;
        }
        // `handleInputFromAbove`, or `handleInputFromBelow` while the fans blow up.
        let rising = chute_motion(lv, pos) > 0.0;
        let (above, from_side, start) = if rising {
            ([pos[0], pos[1] - 1, pos[2]], Direction::Up, 0.0)
        } else {
            ([pos[0], pos[1] + 1, pos[2]], Direction::Down, 1.0)
        };
        // `findEntities` and `extractFromBelt`: a chute the fans lift picks up what lies in
        // the air current under it.
        if rising && lift_from_below(lv, pos, block, powered) {
            continue;
        }
        // `grabCapability`: a chute there hands its items on itself.
        if chute_at(lv, above).is_some() {
            continue;
        }
        // A portable storage interface hands down its contraption's storage while connected;
        // it wakes the chute when it connects.
        let psi = actors::is_item_interface(lv, above);
        let from = items::items_at(lv, above, Some(from_side));
        if from.is_none() && !psi {
            // A chute in a fan's column looks again later: something may drift into the
            // current, or the fan may start pulling it up.
            if rising || fan_on_column(lv, pos) {
                with_world(|w| w.dims[dim].chute_pulls.retry(pos, time));
            }
            continue;
        }
        // Create's own containers say when they change; others are looked at again later.
        let tells = psi
            || matches!(
                info_at(lv, above),
                Some((
                    Block::Depot | Block::WeightedEjector | Block::ItemVault | Block::Chute,
                    _
                )) | Some((Block::SmartChute, _))
            );
        let smart = block == Block::SmartChute;
        let filter = smart
            .then(|| with_world(|w| w.dims[dim].chute_filters.get(&pos).cloned()).flatten())
            .flatten();
        // `getExtractionAmount`, `getExtractionMode`: a smart chute's amount board while it
        // shows (extracting, a filter that stacks).
        let (amount, exact) = if smart {
            super::amount::chute_extraction(lv, pos)
        } else {
            (create_engine::chute::CHUTE_AMOUNT, false)
        };
        let taken = match from {
            _ if psi => {
                let host = &*lv.host;
                actors::psi_extract(
                    lv,
                    above,
                    |s| filters::passes(host, filter.as_ref(), s),
                    amount,
                )
                .ok()
                .flatten()
            }
            Some(from) => items::extract_matching(
                lv,
                &from,
                |host, r| filters::passes(&*host, filter.as_ref(), r.as_stack()),
                amount,
                exact,
            ),
            None => None,
        };
        match taken {
            Some(stack) => {
                set_chute_stack(lv, pos, Some(stack));
                with_world(|w| w.dims[dim].chutes.insert(pos, start));
            }
            None => {
                with_world(|w| {
                    let p = &mut w.dims[dim].chute_pulls;
                    if tells {
                        p.sleep(pos);
                    } else {
                        p.retry(pos, time);
                    }
                });
            }
        }
    }
}

/// Whether an encased fan sits at either end of the column of chutes through `pos`.
fn fan_on_column(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    [1, -1].into_iter().any(|step| {
        let mut p = [pos[0], pos[1] + step, pos[2]];
        while chute_at(lv, p).is_some() {
            p[1] += step;
        }
        info_at(lv, p).is_some_and(|(b, _)| b == Block::EncasedFan)
    })
}

/// `updateAirFlow`'s `bottomPullDistance` for the chute at `pos`: how far below it the current
/// of fans pulling it up (`pull - push`) reaches, up to 3 blocks by speed, none over a chute.
fn bottom_pull(lv: &ServerLevel<'_>, pos: [i32; 3]) -> f32 {
    let speed = chute_pull(lv, pos, 0) - chute_push(lv, pos, 0);
    let max = if speed >= 128.0 {
        3.0
    } else if speed >= 64.0 {
        2.0
    } else if speed >= 32.0 {
        1.0
    } else {
        (speed / 32.0).clamp(0.0, 1.0) * 1.0
    };
    if max <= 0.0 || chute_at(lv, [pos[0], pos[1] - 1, pos[2]]).is_some() {
        return 0.0;
    }
    crate::glue::processing::flow_limit_from(lv, pos, Dir::Down, max).max(0.0)
}

/// `findEntities` then `extractFromBelt` for the empty, rising chute at `pos`: the first item
/// entity it may take in the current under it, else the first item at the middle of a belt
/// segment within reach below. True when it took one.
fn lift_from_below(lv: &mut ServerLevel<'_>, pos: [i32; 3], block: Block, powered: bool) -> bool {
    let dim = lv.dim;
    let reach = bottom_pull(lv, pos);
    if reach > 0.0 {
        let c = pos.map(|v| f64::from(v) + 0.5);
        let d = f64::from(reach);
        let lo = [c[0] - 0.45, c[1] - d - 0.5 - 0.45, c[2] - 0.45];
        let hi = [c[0] + 0.45, c[1] - 0.5 + 0.45, c[2] + 0.45];
        for e in items::items_in(lv, lo, hi) {
            let Some(item) = lv.host.item_entity(e) else {
                continue;
            };
            let Some(stack) = item.stack(&*lv.host) else {
                continue;
            };
            if !chute_accepts(lv, pos, block, powered, &stack) {
                continue;
            }
            // Where the entity is, in the chute's own blocks (below it: negative).
            let y = e
                .position(&*lv.host)
                .map_or(-1.0, |p| (p[1] - f64::from(pos[1])) as f32);
            set_chute_stack(lv, pos, Some(stack));
            with_world(|w| w.dims[dim].chutes.insert(pos, y));
            let _ = e.discard(&mut *lv.host);
            return true;
        }
    }
    // The belt (or depot) up to one block past the current's reach.
    let flow = if reach > 0.0 { reach } else { 0.0 };
    let mut i = 1;
    while i as f32 <= flow + 1.0 {
        let below = [pos[0], pos[1] - i, pos[2]];
        if let Some((c, index, _)) = super::belt::belt_segment(lv, below) {
            let middle = index as f32 + 0.5;
            let found = with_world(|w| {
                let b = w.dims[dim].belts.get(&c)?;
                Some(
                    b.items
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(_, it)| (it.position - middle).abs() < 0.5)
                        .map(|(n, it)| (n, it.clone()))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .unwrap_or_default();
            for (n, it) in found {
                let Some(stack) = super::belt::belt_stack(lv, &it) else {
                    continue;
                };
                if !chute_accepts(lv, pos, block, powered, &stack) {
                    continue;
                }
                let removed = with_world(|w| {
                    let d = &mut w.dims[dim];
                    let b = d.belts.get_mut(&c)?;
                    if b.items.items.get(n) != Some(&it) {
                        return None;
                    }
                    b.items.items.remove(n);
                    d.belt_changes.push(c);
                    Some(())
                })
                .flatten();
                if removed.is_none() {
                    continue;
                }
                set_chute_stack(lv, pos, Some(stack));
                let start = -((i - 1) as f32);
                with_world(|w| w.dims[dim].chutes.insert(pos, start));
                return true;
            }
            return false;
        }
        i += 1;
    }
    false
}

/// A player's click on a smart chute's filter slot (`FilteringBehaviour.onShortInteract` through
/// `ValueSettingsInputHandler`): on a side, not sneaking, at the slot 11 px up; the held item
/// becomes its filter (a filter item taken from the hand, the one it held given back), an empty
/// hand clears it. True when the click was the slot's.
pub fn use_smart_chute(lv: &mut ServerLevel<'_>, pos: [i32; 3], click: &BlockClick<'_>) -> bool {
    let Some(p) = lv.host.player(click.player) else {
        return false;
    };
    // `ValueSettingsInputHandler.canInteract`: may build, not sneaking, no clipboard in hand.
    let sneaking = p.is_secondary_use_active(&*lv.host).unwrap_or(false);
    let held_id = click.stack.map(|s| items::id_of(&*lv.host, s));
    if !p.may_build(&*lv.host).unwrap_or(false)
        || sneaking
        || held_id.as_deref() == Some("create:clipboard")
    {
        return false;
    }
    let Some(side) = Dir::from_offset(click.hit.face) else {
        return false;
    };
    let local = [0, 1, 2].map(|i| click.hit.point[i] - f64::from(pos[i]));
    let Some(slot) = create_engine::chute::filter_slot(side) else {
        return false;
    };
    if !create_engine::board::hits_box(slot, local) {
        return false;
    }
    // `canShortInteract`: a wrench or a mechanical arm is used on the block instead.
    if held_id
        .as_deref()
        .is_some_and(|h| matches!(h, "create:wrench" | "create:mechanical_arm"))
    {
        return false;
    }
    // With its amount board (`acceptsValueSettings`), the client holds the press: its release
    // (a short click) or the board comes on `create:value_settings` (`amount`).
    if super::amount::shown(lv, pos).is_some() {
        return true;
    }
    set_smart_chute_filter(lv, pos, p, click.hand);
    true
}

/// `FilteringBehaviour.onShortInteract` on the smart chute at `pos`: the stack in `hand` becomes
/// the filter (a filter item taken from it outside creative); the one it held goes back, with
/// the frame sound; an exact amount fits the new filter's stack size.
pub(crate) fn set_smart_chute_filter(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    p: mod_api::PlayerRef,
    hand: mod_api::Hand,
) {
    let filter = filters::take_filter(lv.host, p, hand);
    let max = super::amount::filter_max(&*lv.host, filter.as_ref());
    let dim = lv.dim;
    let shown = filter.clone();
    let old = with_world(|w| {
        if let Some(a) = w.dims[dim].filter_amounts.get_mut(&pos) {
            *a = a.fitted(max);
        }
        w.set_chute_filter(dim, pos, filter)
    })
    .flatten();
    super::show_filter(lv, pos, shown.as_ref());
    filters::return_filter(lv, p, pos, old);
}

/// Item vaults whose blocks changed form or split (`updateConnectivity`), and their blocks
/// show it (`large`). Nothing when no vault block changed.
pub fn tick_vaults(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let large = with_world(|w| w.tick_vaults(dim)).unwrap_or_default();
    for (pos, l) in large {
        if matches!(info_at(lv, pos), Some((Block::ItemVault, _))) {
            lv.set_property(
                pos,
                "large",
                if l { "true" } else { "false" },
                UPDATE_CLIENTS,
            );
        }
    }
}
