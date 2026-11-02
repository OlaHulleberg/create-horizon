// Create's seats and turntables in the game, injected as `crate::game::create_seats`: what they
// do to the entities on them. A click on a seat seats the player, a mob landing on one sits
// down (`SeatBlock`), and the seat stand-in goes once it has no rider or its block is gone
// (`SeatEntity.tick`); a turning turntable turns what stands on it (`TurntableBlock.entityInside`,
// `TurntableHandler`). The rules are `create_engine::seat`'s and `create_engine::turntable`'s.
// Seats aren't saved (nor are the game's stand-ins): a mob that sat stands on its seat when the
// world opens again and sits back down. Nothing is looked at each tick without a seat in use or a
// click waiting.
use super::Game;
use super::create::*;
use super::native_entities::NativeEntity;
use create_engine::dye::{self, DyeColor};
use create_engine::world::dimension_index;
use create_engine::{Block, seat, states, turntable};
use horizon_core::position::BlockPos;
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;
use std::collections::HashMap;

/// The game's seat stand-in on a seat block: its entity id, and whether a player sits on it.
#[derive(Clone, Copy, Debug)]
struct Seat {
    id: u32,
    player: bool,
}

/// A player's click on a seat: done at the level's next tick (a player is out of the helpers'
/// reach inside their click), the player found by where they stood.
struct Sit {
    dim: usize,
    pos: [i32; 3],
    at: [f64; 3],
}

thread_local! {
    static SEATS: RefCell<HashMap<(usize, [i32; 3]), Seat>> = RefCell::new(HashMap::new());
    static SITS: RefCell<Vec<Sit>> = const { RefCell::new(Vec::new()) };
}

/// The world closed: its seats went with it.
pub(super) fn forget() {
    SEATS.with_borrow_mut(HashMap::clear);
    SITS.with_borrow_mut(Vec::clear);
}

fn block_of(state: u32) -> Option<Block> {
    states::index(state)?;
    with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
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

/// `SeatBlock.useItemOn` on the server: sneaking passes; a dye of another colour recolours the
/// seat (`BlockHelper.copyProperties`); a seat a player sits on passes; otherwise the clicking
/// player sits down (taking the seat from a mob on it), at the level's next tick. Create's
/// leashed mob sitting in the player's place (`getLeashed`) has no reader.
pub(super) fn use_seat(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    block: Block,
    state: BlockStateId,
    back: bool,
    canceled: &mut bool,
) {
    if back {
        return;
    }
    if let Some(to) = hand_item(game)
        .as_deref()
        .and_then(DyeColor::of_item)
        .and_then(|c| dye::recoloured(block, c))
    {
        *canceled = true;
        let registry = game.pack.assets.registry;
        let Some(mut s) = registry.state(state).cloned() else {
            return;
        };
        s.name = format!("create:{}", to.name());
        if let Some(id) = registry.id(&s) {
            let dimension = game.dim.id().to_owned();
            game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
        }
        return;
    }
    if SEATS.with_borrow(|s| s.get(&(dim, pos)).is_some_and(|s| s.player)) {
        return;
    }
    let Some(player) = game.native_acting_player() else {
        return;
    };
    *canceled = true;
    SITS.with_borrow_mut(|s| {
        s.push(Sit {
            dim,
            pos,
            at: player.position,
        })
    });
}

/// The player standing at `at` (where a click's player stood), nearest it.
fn player_at(game: &Game<'_>, dimension: &str, at: [f64; 3]) -> Option<NativeEntity> {
    let distance = |e: &NativeEntity| (0..3).map(|a| (e.pos[a] - at[a]).powi(2)).sum::<f64>();
    game.native_entities_in(dimension, at.map(|v| v - 1.0), at.map(|v| v + 1.0))
        .into_iter()
        .filter(|e| e.kind == "minecraft:player")
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
}

/// `SeatBlock.sitDown`: a seat at the block, `rider` on it (off what it rode first, as
/// `startRiding` does). Whether it sat.
fn sit_down(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    rider: &NativeEntity,
) -> bool {
    if rider.vehicle.is_some() {
        game.native_stop_riding(dimension, rider.id);
    }
    let Some(id) = game.native_spawn_seat(dimension, seat::seat_at(pos, &rider.kind), 0.0) else {
        return false;
    };
    if !game.native_start_riding(dimension, rider.id, id) {
        game.native_remove_seat(dimension, id);
        return false;
    }
    let player = rider.kind == "minecraft:player";
    SEATS.with_borrow_mut(|s| s.insert((dim, pos), Seat { id, player }));
    true
}

/// `Contraption.moveSeat`: the seat block at `pos` moves into a contraption (a train's
/// carriage); its seat's first rider is taken off and returned, and the seat goes.
pub(super) fn take_rider(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
) -> Option<NativeEntity> {
    let seat = SEATS.with_borrow_mut(|s| s.remove(&(dim, pos)))?;
    let rider = game.native_passengers(dimension, seat.id).first().copied();
    game.native_remove_seat(dimension, seat.id);
    game.native_entity(dimension, rider?)
}

/// `Contraption.addPassengersToWorld`: a contraption's rider sits on the seat block at `pos`
/// put back in the world, if it is a seat and free (`isSeatOccupied`, `sitDown`).
pub(super) fn seat_rider(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    rider: &NativeEntity,
) -> bool {
    let is_seat = block_of(game.world.block(pos[0], pos[1], pos[2]).0).is_some_and(dye::is_seat);
    if !is_seat || SEATS.with_borrow(|s| s.contains_key(&(dim, pos))) {
        return false;
    }
    sit_down(game, dimension, dim, pos, rider)
}

/// The seat at `pos` lets its riders off where Create's do (`getDismountLocationForPassenger`)
/// and goes.
fn remove_seat(game: &mut Game<'_>, dimension: &str, pos: [i32; 3], id: u32) {
    game.native_move_seat(dimension, id, seat::dismount_at(pos), 0.0);
    game.native_remove_seat(dimension, id);
}

/// After a server level's tick: the players who clicked a seat sit down (a mob on it gets off
/// first), then each seat in use goes when it has no rider or its block isn't a seat any more
/// (`SeatEntity.tick`).
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    if SEATS.with_borrow(HashMap::is_empty) && SITS.with_borrow(Vec::is_empty) {
        return;
    }
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let sits: Vec<Sit> = SITS.with_borrow_mut(|s| {
        let (here, rest) = std::mem::take(s).into_iter().partition(|s| s.dim == dim);
        *s = rest;
        here
    });
    let is_seat = |game: &Game<'_>, pos: [i32; 3]| {
        block_of(game.world.block(pos[0], pos[1], pos[2]).0).is_some_and(dye::is_seat)
    };
    for sit in sits {
        if !is_seat(game, sit.pos) {
            continue;
        }
        let Some(player) = player_at(game, dimension, sit.at) else {
            continue;
        };
        let Some(seat) = SEATS.with_borrow(|s| s.get(&(dim, sit.pos)).copied()) else {
            sit_down(game, dimension, dim, sit.pos, &player);
            continue;
        };
        if seat.player || player.vehicle == Some(seat.id) {
            continue;
        }
        // `seatEntity.ejectPassengers(); player.startRiding(seatEntity)`, at a player's height.
        for rider in game.native_passengers(dimension, seat.id) {
            game.native_stop_riding(dimension, rider);
        }
        if player.vehicle.is_some() {
            game.native_stop_riding(dimension, player.id);
        }
        let at = seat::seat_at(sit.pos, "minecraft:player");
        game.native_move_seat(dimension, seat.id, at, 0.0);
        if game.native_start_riding(dimension, player.id, seat.id) {
            SEATS.with_borrow_mut(|s| {
                if let Some(s) = s.get_mut(&(dim, sit.pos)) {
                    s.player = true;
                }
            });
        }
    }
    let seats: Vec<([i32; 3], Seat)> = SEATS.with_borrow(|s| {
        s.iter()
            .filter(|((d, _), _)| *d == dim)
            .map(|((_, p), s)| (*p, *s))
            .collect()
    });
    for (pos, s) in seats {
        if is_seat(game, pos) && !game.native_passengers(dimension, s.id).is_empty() {
            continue;
        }
        remove_seat(game, dimension, pos, s.id);
        SEATS.with_borrow_mut(|m| m.remove(&(dim, pos)));
    }
}

/// `BlockBehaviour.entityInside` for Create's blocks with `entity_inside`: seats and turntables.
/// Hot: one range test, then return, for any other block.
pub(super) fn entity_inside(
    game: &mut Game<'_>,
    dimension: &str,
    pos: BlockPos,
    state: BlockStateId,
    entity: &NativeEntity,
) {
    let Some(block) = block_of(state.0) else {
        return;
    };
    let at = [pos.x, pos.y, pos.z];
    if dye::is_seat(block) {
        seat_inside(game, dimension, at, entity);
    } else if block == Block::Turntable {
        turntable_inside(game, dimension, at, state, entity);
    }
}

/// `SeatBlock.updateEntityAfterFallOn`: a living mob that lands on a free seat, standing in its
/// block, sits down.
fn seat_inside(game: &mut Game<'_>, dimension: &str, pos: [i32; 3], e: &NativeEntity) {
    if e.vehicle.is_some() || e.kind == "minecraft:player" {
        return;
    }
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let occupied = SEATS.with_borrow(|s| s.contains_key(&(dim, pos)));
    if !seat::sits_down(pos, &e.kind, e.pos, e.on_ground, false, occupied)
        || game.native_mob(dimension, e.id).is_none()
    {
        return;
    }
    sit_down(game, dimension, dim, pos, e);
}

/// `TurntableBlock.entityInside` and `TurntableHandler` on a turntable turning at its speed: an
/// entity standing on it turns by 3/10 of the speed a tick (a mob's head with it); a player
/// standing on it (`getOnPos`) turns as their client turns them, and one whose middle is off it
/// is carried round it (`setDeltaMovement` on the server's copy of their motion, their client
/// told). Create's `setNoActionTime(20)` and `setOnGround(false)` for mobs have no writer.
fn turntable_inside(
    game: &mut Game<'_>,
    dimension: &str,
    pos: [i32; 3],
    state: BlockStateId,
    e: &NativeEntity,
) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let rpm = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
    let speed = turntable::mob_turn(rpm);
    if speed == 0.0 {
        return;
    }
    let stands = turntable::stands_on(pos, e.pos, e.on_ground, e.velocity[1]);
    if e.kind != "minecraft:player" {
        if stands {
            game.native_set_yaw(dimension, e.id, e.yaw - speed);
        }
        return;
    }
    let below = [e.pos[0], e.pos[1] - 1.0e-5, e.pos[2]].map(|v| v.floor() as i32);
    if e.on_ground && below == pos {
        let turn = turntable::player_turn(rpm, pos, e.pos);
        if turn != 0.0 {
            game.native_set_yaw(dimension, e.id, e.yaw - turn);
        }
    }
    let middle = e.pos.map(|v| v.floor() as i32);
    if stands && game.world.block(middle[0], middle[1], middle[2]).0 != state.0 {
        let m = turntable::carry(pos, e.pos, speed);
        let v = e.velocity;
        game.native_set_delta_movement(dimension, e.id, [v[0] + m[0], v[1] + m[1], v[2] + m[2]]);
    }
}

/// NeoForge's `EntityMountEvent` for a player sneaking off a seat: the seat moves to where
/// Create lets its rider off (`getDismountLocationForPassenger`) before the game puts them where
/// it is; it goes at the level's next tick, riderless.
pub(super) fn entity_mount(game: &mut Game<'_>, dimension: &str, vehicle: u32, mounting: bool) {
    if mounting || SEATS.with_borrow(HashMap::is_empty) {
        return;
    }
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let pos = SEATS.with_borrow(|s| {
        s.iter()
            .find(|((d, _), s)| *d == dim && s.id == vehicle)
            .map(|((_, p), _)| *p)
    });
    if let Some(pos) = pos {
        game.native_move_seat(dimension, vehicle, seat::dismount_at(pos), 0.0);
    }
}
