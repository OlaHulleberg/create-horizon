// Game glue for Create's blocks that a vanilla template mostly runs, injected as
// `crate::game::create_blocks`: valve handles (a crank turned by an angle), sliding doors (the
// iron door's template with Create's double doors and `visible`), toolboxes (a container with a
// menu) and postboxes (`open` while their menu is). Rules live in `create_engine`.

use super::Game;
use super::create::*;
use create_engine::Block;
use create_engine::door::{Door, Slide};
use create_engine::dye::{self, DyeColor};
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;

thread_local! {
    /// Postbox menus a player's click asked for (player, dimension index, position), opened at
    /// the level's next tick (a menu can't open while the player's state is swapped in); and
    /// postboxes whose last menu closed, shut at their level's next tick.
    static POSTBOX_OPENS: RefCell<Vec<(u128, usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
    static POSTBOX_CLOSES: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
}

/// The item in the hand the click uses.
fn hand_item(game: &Game<'_>) -> Option<String> {
    let stack = if game.hand == horizon_client::first_person::Hand::Off {
        game.offhand.as_ref()
    } else {
        game.inventory.get(game.selected).and_then(Option::as_ref)
    };
    stack.map(|s| s.item.to_string())
}

/// Whether the player holds anything (`ServerPlayerGameMode.useItemOn`: sneaking with an item
/// skips the block's use).
fn holds_something(game: &Game<'_>) -> bool {
    game.offhand.is_some()
        || game
            .inventory
            .get(game.selected)
            .is_some_and(Option::is_some)
}

/// The state `state` with properties `props` changed.
fn with_props(
    game: &Game<'_>,
    state: BlockStateId,
    props: &[(&str, &str)],
) -> Option<BlockStateId> {
    let registry = game.pack.assets.registry;
    let mut s = registry.state(state)?.clone();
    for (k, v) in props {
        s.properties.insert((*k).to_owned(), (*v).to_owned());
    }
    registry.id(&s)
}

/// `BlockHelper.copyProperties(state, to.getDefaultState())`: the block `to` with the state's
/// properties.
fn recoloured_state(game: &Game<'_>, state: BlockStateId, to: Block) -> Option<BlockStateId> {
    let registry = game.pack.assets.registry;
    let mut s = registry.state(state)?.clone();
    s.name = format!("create:{}", to.name());
    registry.id(&s)
}

/// A right click on one of these blocks, on the server: true when it is one of them (handled or
/// passed on to the template as Create does).
#[allow(clippy::too_many_arguments)]
pub(super) fn use_block(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    block: Block,
    state: BlockStateId,
    back: bool,
    canceled: &mut bool,
) -> bool {
    if dye::is_valve_handle(block) {
        use_valve_handle(game, dim, pos, block, state, back, canceled);
    } else if dye::is_sliding_door(block) {
        use_door(game, pos, block, state, back, canceled);
    } else if dye::is_toolbox(block) {
        use_toolbox(game, pos, block, state, back, canceled);
    } else if dye::is_postbox(block) {
        use_postbox(game, dim, pos, back, canceled);
    } else if matches!(block, Block::TrainTrapdoor | Block::FramedGlassTrapdoor) {
        use_trapdoor(game, pos, state, back, canceled);
    } else {
        return false;
    }
    true
}

/// `ValveHandleBlock.onBlockActivated` → `clicked`: not for a player who may not build, nor a
/// sneaking wrench; a dye of another colour recolours it (`DYED_VALVE_HANDLES`, its properties
/// kept), else it turns (`activate`). The click is taken either way.
fn use_valve_handle(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    block: Block,
    state: BlockStateId,
    back: bool,
    canceled: &mut bool,
) {
    if !game.may_build() {
        return;
    }
    let item = hand_item(game);
    if back && item.as_deref() == Some("create:wrench") {
        return;
    }
    *canceled = true;
    if let Some(to) = item
        .as_deref()
        .and_then(DyeColor::of_item)
        .and_then(|c| dye::recoloured(block, c))
    {
        if let Some(id) = recoloured_state(game, state, to) {
            let dimension = game.dim.id().to_owned();
            game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
        }
        return;
    }
    with_world(|w| w.use_valve(dim, pos, back));
}

/// `ToolboxBlock.useItemOn`: sneaking passes (empty-handed, nothing opens); a dye of another
/// colour recolours it, its items kept (`onRemove` keeps the block entity for another
/// toolbox); else its menu opens (the template's use).
fn use_toolbox(
    game: &mut Game<'_>,
    pos: [i32; 3],
    block: Block,
    state: BlockStateId,
    back: bool,
    canceled: &mut bool,
) {
    if back {
        if !holds_something(game) {
            *canceled = true;
        }
        return;
    }
    let Some(to) = hand_item(game)
        .as_deref()
        .and_then(DyeColor::of_item)
        .and_then(|c| dye::recoloured(block, c))
    else {
        return;
    };
    *canceled = true;
    let Some(id) = recoloured_state(game, state, to) else {
        return;
    };
    let items = game
        .native_with_mod_block_entity(pos, |be| std::mem::take(&mut be.items))
        .unwrap_or_default();
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
    game.native_with_mod_block_entity(pos, |be| be.items = items);
}

/// `PostboxBlock.useWithoutItem` → `PackagePortBlockEntity.use`: the main hand's click, not
/// sneaking, opens its menu (18 slots, for packages; the clipboard's address and the packages
/// themselves are the package subsystem's).
fn use_postbox(game: &mut Game<'_>, dim: usize, pos: [i32; 3], back: bool, canceled: &mut bool) {
    if back || game.hand != horizon_client::first_person::Hand::Main {
        return;
    }
    *canceled = true;
    if let Some(p) = game.native_acting_player() {
        POSTBOX_OPENS.with_borrow_mut(|o| o.push((p.uuid, dim, pos)));
    }
}

/// A player's menu closed: a postbox whose last menu it was shuts at its level's next tick.
pub(super) fn menu_closed(uuid: u128, window: i32) {
    if let Some(Some(at)) = with_world(|w| w.postboxes.closed(uuid, window)) {
        POSTBOX_CLOSES.with_borrow_mut(|c| c.push(at));
    }
}

/// `PostboxBlockEntity.onOpenChange`: `open` set (`setBlockAndUpdate`) with a barrel's sound, if
/// the block is still a postbox.
fn set_postbox_open(game: &mut Game<'_>, dimension: &str, pos: [i32; 3], open: bool) {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    if with_world(|w| w.table.get(state).is_some_and(|(b, _)| dye::is_postbox(b))) != Some(true) {
        return;
    }
    let value = if open { "true" } else { "false" };
    game.native_set_block_property(pos, "open", value);
    let sound = if open { "open" } else { "close" };
    let [x, y, z] = pos.map(|v| f64::from(v) + 0.5);
    game.native_run_command(&format!(
        "execute in {dimension} run playsound minecraft:block.barrel.{sound} blocks @a {x} {y} {z} 1 1"
    ));
}

/// The postboxes of level `dimension` to open and shut.
pub(super) fn tick_postboxes(game: &mut Game<'_>, dimension: &str, dim: usize) {
    if POSTBOX_OPENS.with_borrow(Vec::is_empty) && POSTBOX_CLOSES.with_borrow(Vec::is_empty) {
        return;
    }
    let closes: Vec<[i32; 3]> = POSTBOX_CLOSES.with_borrow_mut(|c| {
        let mine = c
            .iter()
            .filter(|(d, _)| *d == dim)
            .map(|(_, p)| *p)
            .collect();
        c.retain(|(d, _)| *d != dim);
        mine
    });
    for pos in closes {
        set_postbox_open(game, dimension, pos, false);
    }
    let opens: Vec<(u128, [i32; 3])> = POSTBOX_OPENS.with_borrow_mut(|o| {
        let mine = o
            .iter()
            .filter(|(_, d, _)| *d == dim)
            .map(|(u, _, p)| (*u, *p))
            .collect();
        o.retain(|(_, d, _)| *d != dim);
        mine
    });
    for (uuid, pos) in opens {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let Some(block) = with_world(|w| {
            w.table
                .get(state)
                .map(|(b, _)| b)
                .filter(|b| dye::is_postbox(*b))
        })
        .flatten() else {
            continue;
        };
        let title =
            horizon_client::text::Text::translate(&format!("block.create.{}", block.name()), &[]);
        let Some(window) = game.native_open_menu(
            uuid,
            2,
            title,
            super::create_packages::port_stacks(dim, pos),
        ) else {
            continue;
        };
        let Some((first, left)) = with_world(|w| w.postboxes.opened(uuid, window, dim, pos)) else {
            continue;
        };
        if let Some(at) = left {
            POSTBOX_CLOSES.with_borrow_mut(|c| c.push(at));
        }
        if first {
            set_postbox_open(game, dimension, pos, true);
        }
    }
}

/// `TrainTrapdoorBlock.useWithoutItem`: a click (not sneaking with something held) opens or
/// closes it whatever its power (`state.cycle(OPEN)`, `UPDATE_CLIENTS`; a waterlogged one has
/// its water ticked by the template), with its `BlockSetType`'s iron trapdoor sounds.
fn use_trapdoor(
    game: &mut Game<'_>,
    pos: [i32; 3],
    state: BlockStateId,
    back: bool,
    canceled: &mut bool,
) {
    if back && holds_something(game) {
        return;
    }
    let Some(open) = with_world(|w| w.table.get(state.0).map(|(_, i)| i.open)).flatten() else {
        return;
    };
    *canceled = true;
    let open = !open;
    let Some(id) = with_props(
        game,
        state,
        &[("open", if open { "true" } else { "false" })],
    ) else {
        return;
    };
    let dimension = game.dim.id().to_owned();
    game.native_set_block(
        &dimension,
        pos,
        id,
        super::native_hooks::flags::UPDATE_CLIENTS,
    );
    let sound = if open {
        "minecraft:block.iron_trapdoor.open"
    } else {
        "minecraft:block.iron_trapdoor.close"
    };
    let pitch = door_pitch(game, pos);
    let [x, y, z] = pos.map(|v| f64::from(v) + 0.5);
    game.native_run_command(&format!(
        "execute in {dimension} run playsound {sound} blocks @a {x} {y} {z} 1 {pitch}"
    ));
}

/// The sliding door at `pos`: its block and state.
fn door_at(game: &Game<'_>, pos: [i32; 3]) -> Option<(Block, Door, BlockStateId)> {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    with_world(|w| {
        let (b, info) = w.table.get(state.0)?;
        dye::is_sliding_door(b)
            .then(|| Door::of(info))
            .flatten()
            .map(|d| (b, d, state))
    })
    .flatten()
}

/// Both halves of the door at `pos` set `open` (and hidden while open), as `setBlock` of one
/// half and the other's `updateShape` (which copies it, `visible` too) leave them.
fn set_door_open(game: &mut Game<'_>, dimension: &str, pos: [i32; 3], door: &Door, open: bool) {
    for at in [pos, door.other_half(pos)] {
        let Some((_, _, state)) = door_at(game, at) else {
            continue;
        };
        let props: &[(&str, &str)] = if open {
            &[("open", "true"), ("visible", "false")]
        } else {
            &[("open", "false")]
        };
        if let Some(id) = with_props(game, state, props).filter(|id| *id != state) {
            // UPDATE_CLIENTS | UPDATE_IMMEDIATE
            game.native_set_block(dimension, at, id, 10);
        }
    }
}

/// A pitch of `random.nextFloat() * 0.1 + 0.9`; any draw will do.
fn door_pitch(game: &Game<'_>, pos: [i32; 3]) -> f32 {
    let seed = (game.world.game_time as u64)
        ^ (pos[0] as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (pos[2] as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
        ^ (pos[1] as u64);
    0.9 + (seed.wrapping_mul(6_364_136_223_846_793_005) >> 40) as f32 / (1u64 << 24) as f32 * 0.1
}

/// `SlidingDoorBlock.useWithoutItem`: the main hand's click (not sneaking with something held)
/// opens or closes the door; a double door's other door follows (`tick_doors`), and an iron
/// door's opening sound plays at the last door it reached.
fn use_door(
    game: &mut Game<'_>,
    pos: [i32; 3],
    block: Block,
    state: BlockStateId,
    back: bool,
    canceled: &mut bool,
) {
    if game.hand != horizon_client::first_person::Hand::Main || (back && holds_something(game)) {
        return;
    }
    let Some(door) = with_world(|w| w.table.get(state.0).and_then(|(_, i)| Door::of(i))).flatten()
    else {
        return;
    };
    *canceled = true;
    let open = !door.open;
    let dimension = game.dim.id().to_owned();
    set_door_open(game, &dimension, pos, &door, open);
    if !open {
        return;
    }
    let partner = door.partner(pos);
    // `isDoubleDoor(state.cycle(OPEN), ...)`: the other door not yet as open as this one now.
    let double = door_at(game, partner)
        .is_some_and(|(b, other, _)| b == block && Door { open, ..door }.is_double(&other));
    let at = if double { partner } else { pos };
    let pitch = door_pitch(game, at);
    let [x, y, z] = at.map(|v| f64::from(v) + 0.5);
    game.native_run_command(&format!(
        "execute in {dimension} run playsound minecraft:block.iron_door.open blocks @a {x} {y} {z} 1 {pitch}"
    ));
}

/// Sliding doors after the level's tick: a door that opened is hidden (`visible` false, as
/// Create sets it with `open`) and takes the other door of a double door with it, and its panel
/// slides; once a closed door's panel has slid shut it shows again, with an iron door's
/// closing sound. Only doors that changed or still slide cost anything.
pub(super) fn tick_doors(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let idle = |w: &mut create_engine::world::CreateWorld| {
        w.dims[dim].door_changes.is_empty() && w.dims[dim].doors.is_empty()
    };
    if with_world(idle) != Some(false) {
        return;
    }
    // A double door's other door changes in this pass too.
    for _ in 0..4 {
        let changes =
            with_world(|w| std::mem::take(&mut w.dims[dim].door_changes)).unwrap_or_default();
        if changes.is_empty() {
            break;
        }
        for (pos, was_open) in changes {
            let Some((block, door, state)) = door_at(game, pos) else {
                continue;
            };
            if door.open
                && door.visible
                && let Some(id) = with_props(game, state, &[("visible", "false")])
            {
                game.native_set_block(dimension, pos, id, 10);
            }
            let partner = door.partner(pos);
            if let Some((b, other, _)) = door_at(game, partner)
                && b == block
                && door.is_double(&other)
            {
                set_door_open(game, dimension, partner, &other, door.open);
            }
            if !door.upper {
                with_world(|w| {
                    w.dims[dim]
                        .doors
                        .entry(pos)
                        .or_insert_with(|| Slide::at_rest(was_open));
                });
            }
        }
    }
    let moving: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].doors.keys().copied().collect()).unwrap_or_default();
    for pos in moving {
        let Some((_, door, state)) = door_at(game, pos).filter(|(_, d, _)| !d.upper) else {
            with_world(|w| w.dims[dim].doors.remove(&pos));
            continue;
        };
        let Some((show, done)) = with_world(|w| {
            let s = w.dims[dim].doors.get_mut(&pos)?;
            let show = s.tick(door.open, door.visible);
            Some((show, s.at_target()))
        })
        .flatten() else {
            continue;
        };
        if show {
            // `showBlockModel`: UPDATE_ALL, the upper half following through `updateShape`.
            for (at, s) in [(pos, Some(state)), (door.other_half(pos), None)] {
                let s = s.or_else(|| door_at(game, at).map(|(_, _, s)| s));
                if let Some(id) = s.and_then(|s| with_props(game, s, &[("visible", "true")])) {
                    game.native_set_block(
                        dimension,
                        at,
                        id,
                        super::native_hooks::flags::UPDATE_ALL,
                    );
                }
            }
            let [x, y, z] = pos.map(|v| f64::from(v) + 0.5);
            game.native_run_command(&format!(
                "execute in {dimension} run playsound minecraft:block.iron_door.close blocks @a {x} {y} {z} 0.5 1"
            ));
        }
        if done {
            with_world(|w| w.dims[dim].doors.remove(&pos));
        }
    }
}
