//! Create's seats and turntables: what they do to the entities on them. A click on a seat seats
//! the player, a mob landing on one sits down (`SeatBlock`), and the seat stand-in goes once it
//! has no rider or its block is gone (`SeatEntity.tick`); a turning turntable turns what stands
//! on it (`TurntableBlock.entityInside`, `TurntableHandler`). The rules are
//! `create_engine::seat`'s and `create_engine::turntable`'s.
//!
//! The seat's own saved general entity is invisible and not pickable. Its passenger point
//! sets the rider's height. A seat read back with its chunk or its player is claimed by the
//! block it stands on.
//!
//! `entity_inside` runs inside the entity's move, where its position, ground and motion can't
//! be read: the entity is noted there and seated or turned at the end of the level's tick.
//! A player clicking a free seat while leading a mob seats the mob instead (`getLeashed`). A
//! seat bounces what won't sit on it (`bounce_restitution`) and halves the fall it breaks
//! (`SeatBlock.fallOn`). Create's `setNoActionTime(20)` and `setOnGround(false)` for mobs on a
//! turntable have no writer (`API_GAPS.md`).

use super::super::server::{ServerLevel, update_all};
use super::super::states::key;
use super::super::subsystem::Subsystem;
use super::super::world::{level_index, with_world};
use super::{clicker, in_click_level};
use create_engine::Block;
use create_engine::dye::{self, DyeColor};
use create_engine::{seat, turntable};
use mod_api::{
    BlockPart, ClientHost, EntityInsideEvent, EntityJoinLevelEvent, EntityMountEvent, EntityRef,
    EntitySpawn, FallOnEvent, Flow, GeneralEntityCategory, GeneralSavePolicy, InteractionResult,
    LeashHolder, LevelRandomHost, Registrar, RegistrationContext, RemovalReason, ServerHost,
    SpawnBody, UseItemOnEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;

pub(crate) const ENTITY_TYPE: &str = "create:seat";

/// The game's seat stand-in on a seat block: its entity id, and whether a player sits on it.
#[derive(Clone, Copy, Debug)]
struct Seat {
    id: u32,
    player: bool,
}

thread_local! {
    static SEATS: RefCell<HashMap<(usize, [i32; 3]), Seat>> = RefCell::new(HashMap::new());
    /// Players' clicks on seats (dimension, seat, player), for the level's tick.
    static SITS: RefCell<Vec<(usize, [i32; 3], u128)>> = const { RefCell::new(Vec::new()) };
    /// Stand-ins that came back (dimension, id), for the level's tick.
    static LOADED: RefCell<Vec<(usize, u32)>> = const { RefCell::new(Vec::new()) };
    /// Entities inside a seat or turntable this tick (dimension, block, entity).
    static INSIDE: RefCell<Vec<(usize, [i32; 3], u32)>> = const { RefCell::new(Vec::new()) };
}

pub struct Seats;

impl Subsystem for Seats {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        if let Err(e) = r.declare_general_entity(
            "seat",
            GeneralEntityCategory::Misc,
            0.25,
            f64::from(seat::HEIGHT),
            Some([0.0, seat::PASSENGER_HEIGHT, 0.0]),
            GeneralSavePolicy::Save,
            true,
            false,
            5,
            i32::MAX,
            1,
            &[],
        ) {
            crate::create_log!("[create] seat type was refused: {e:?}");
        }
        for b in dye::SEATS {
            r.block_part(key(b), SeatPart);
        }
        r.block_part(key(Block::Turntable), SeatPart);
    }

    fn world_closed(&self) {
        SEATS.with_borrow_mut(HashMap::clear);
        SITS.with_borrow_mut(Vec::clear);
        LOADED.with_borrow_mut(Vec::clear);
        INSIDE.with_borrow_mut(Vec::clear);
    }

    /// A seat stand-in came back with its riders: taken back at the level's next tick.
    fn entity_join_level(
        &self,
        _host: &mut dyn ServerHost,
        event: &mut EntityJoinLevelEvent<'_>,
    ) -> Flow {
        if event.kind == ENTITY_TYPE
            && let Some(dim) = level_index(event.dimension)
        {
            LOADED.with_borrow_mut(|l| l.push((dim, event.entity)));
        }
        Flow::Continue
    }

    /// NeoForge's `EntityMountEvent` for a rider getting off a seat: the seat moves to where
    /// Create lets its rider off (`getDismountLocationForPassenger`) before the game puts them
    /// where it is; it goes at the level's next tick, riderless.
    fn entity_mount(&self, host: &mut dyn ServerHost, event: &mut EntityMountEvent<'_>) -> Flow {
        if event.mounting || SEATS.with_borrow(HashMap::is_empty) {
            return Flow::Continue;
        }
        let Some(dim) = level_index(event.dimension) else {
            return Flow::Continue;
        };
        let pos = SEATS.with_borrow(|s| {
            s.iter()
                .find(|((d, _), s)| *d == dim && s.id == event.vehicle)
                .map(|((_, p), _)| *p)
        });
        if let Some(pos) = pos
            && let Some(seat) = host.entity(event.vehicle)
        {
            seat.teleport_to(host, seat::dismount_at(pos), None);
        }
        Flow::Continue
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        level_tick(lv);
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        turn_local_player(host);
    }
}

/// `EntityNameDisplaySource`: the name of the first one riding the seat at `pos`
/// (`getDisplayName`), if anyone does.
pub(crate) fn rider_name(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<String> {
    let dim = lv.dim;
    let seat = SEATS.with_borrow(|s| s.get(&(dim, pos)).copied())?;
    let stand = lv.host.entity(seat.id)?;
    let rider = stand.passengers(&*lv.host)?.into_iter().next()?;
    rider.name(&*lv.host)
}

/// The mob (not a player) sitting on the seat block at `pos`, if any: a train assembled with
/// the seat takes it along (`Contraption.addPassengersToWorld` / `getSeatMapping`).
pub fn seated_mob(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<EntityRef> {
    let seat = SEATS.with_borrow(|s| s.get(&(lv.dim, pos)).map(|s| (s.id, s.player)))?;
    if seat.1 {
        return None;
    }
    let stand = lv.host.entity(seat.0)?;
    let riders = stand.passengers(&*lv.host)?;
    riders
        .into_iter()
        .find(|r| kind_of(lv, *r) != "minecraft:player")
}

/// `SeatBlock.getLeashed`: a mob within 10 blocks of the player's box that the player holds on
/// a leash and that may sit (`canBePickedUp`), which sits in the player's place.
fn leashed(lv: &ServerLevel<'_>, player: EntityRef) -> Option<EntityRef> {
    let host = &*lv.host;
    let [min, max] = player.bounding_box(host)?;
    let near = lv
        .level
        .entities_in(host, min.map(|v| v - 10.0), max.map(|v| v + 10.0))?;
    near.into_iter().find(|e| {
        e.id() != player.id()
            && host.mob(*e).is_some()
            && seat::can_sit(&kind_of(lv, *e))
            && matches!(e.leash_holder(host), Some(LeashHolder::Entity(h)) if h.id() == player.id())
    })
}

fn is_seat(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    lv.create_block(pos).is_some_and(dye::is_seat)
}

fn kind_of(lv: &ServerLevel<'_>, e: EntityRef) -> String {
    e.kind(&*lv.host)
        .map(|k| k.id(&*lv.host))
        .unwrap_or_default()
}

/// `SeatBlock.sitDown`: a seat at the block, `rider` on it (off what it rode first, as
/// `startRiding` does). Whether it sat.
fn sit_down(lv: &mut ServerLevel<'_>, pos: [i32; 3], rider: EntityRef) -> bool {
    if rider.vehicle(&*lv.host).is_some() {
        rider.stop_riding(&mut *lv.host);
    }
    let kind = kind_of(lv, rider);
    let Some(seat) = lv.level.spawn_entity(
        &mut *lv.host,
        ENTITY_TYPE,
        seat::seat_anchor(pos, &kind),
        &EntitySpawn::new(SpawnBody::General),
    ) else {
        return false;
    };
    if !rider.start_riding(&mut *lv.host, seat, true).is_ok() {
        seat.discard(&mut *lv.host);
        return false;
    }
    let player = kind == "minecraft:player";
    SEATS.with_borrow_mut(|s| {
        s.insert(
            (lv.dim, pos),
            Seat {
                id: seat.id(),
                player,
            },
        )
    });
    true
}

/// Claim the seats whose blocks still exist (`SeatEntity.tick`). Retire unclaimed carriers
/// and leave their riders and any already-claimed seat in the world.
fn take_back(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let loaded: Vec<u32> = LOADED.with_borrow_mut(|l| {
        let mine = l.iter().filter(|e| e.0 == dim).map(|e| e.1).collect();
        l.retain(|e| e.0 != dim);
        mine
    });
    for id in loaded {
        if SEATS.with_borrow(|s| s.values().any(|s| s.id == id)) {
            continue;
        }
        let Some(e) = lv.host.entity(id) else {
            continue;
        };
        let Some(at) = e.position(&*lv.host) else {
            continue;
        };
        let pos = at.map(|v| v.floor() as i32);
        let free = is_seat(lv, pos) && !SEATS.with_borrow(|s| s.contains_key(&(dim, pos)));
        if !free {
            // Removing a general entity dismounts its passengers without removing them.
            e.remove(&mut *lv.host, RemovalReason::Discarded);
            continue;
        }
        // A seat's riders are players or mobs.
        let riders = e.passengers(&*lv.host).unwrap_or_default();
        let player = riders.iter().any(|&r| kind_of(lv, r) == "minecraft:player");
        SEATS.with_borrow_mut(|s| s.insert((dim, pos), Seat { id, player }));
    }
}

/// After a server level's tick: stand-ins that came back are taken back, the players who
/// clicked a seat sit down (a mob on it gets off first), mobs that landed on a free seat sit,
/// turntables turn what stands on them, then each seat in use goes when it has no rider or its
/// block isn't a seat any more (`SeatEntity.tick`).
fn level_tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    if SEATS.with_borrow(HashMap::is_empty)
        && SITS.with_borrow(Vec::is_empty)
        && LOADED.with_borrow(Vec::is_empty)
        && INSIDE.with_borrow(Vec::is_empty)
    {
        return;
    }
    take_back(lv);
    let sits: Vec<([i32; 3], u128)> = SITS.with_borrow_mut(|s| {
        let mine = s
            .iter()
            .filter(|e| e.0 == dim)
            .map(|e| (e.1, e.2))
            .collect();
        s.retain(|e| e.0 != dim);
        mine
    });
    for (pos, uuid) in sits {
        if !is_seat(lv, pos) {
            continue;
        }
        let Some(player) = lv.host.player(uuid).map(|p| p.entity()) else {
            continue;
        };
        let Some(seat) = SEATS.with_borrow(|s| s.get(&(dim, pos)).copied()) else {
            let rider = leashed(lv, player).unwrap_or(player);
            sit_down(lv, pos, rider);
            continue;
        };
        let riding = player.vehicle(&*lv.host).map(|v| v.id());
        if seat.player || riding == Some(seat.id) {
            continue;
        }
        let Some(stand) = lv.host.entity(seat.id) else {
            continue;
        };
        // `seatEntity.ejectPassengers(); player.startRiding(seatEntity)`, at a player's height.
        stand.eject_passengers(&mut *lv.host);
        if riding.is_some() {
            player.stop_riding(&mut *lv.host);
        }
        stand.teleport_to(
            &mut *lv.host,
            seat::seat_anchor(pos, "minecraft:player"),
            None,
        );
        if player.start_riding(&mut *lv.host, stand, true).is_ok() {
            SEATS.with_borrow_mut(|s| {
                if let Some(s) = s.get_mut(&(dim, pos)) {
                    s.player = true;
                }
            });
        }
    }
    let inside: Vec<([i32; 3], u32)> = INSIDE.with_borrow_mut(|i| {
        let mut mine: Vec<_> = i
            .iter()
            .filter(|e| e.0 == dim)
            .map(|e| (e.1, e.2))
            .collect();
        i.retain(|e| e.0 != dim);
        mine.sort();
        mine.dedup();
        mine
    });
    for (pos, id) in inside {
        let Some(e) = lv.host.entity(id) else {
            continue;
        };
        match lv.create_block(pos) {
            Some(b) if dye::is_seat(b) => seat_inside(lv, pos, e),
            Some(Block::Turntable) => turntable_inside(lv, pos, e),
            _ => {}
        }
    }
    let seats: Vec<([i32; 3], Seat)> = SEATS.with_borrow(|s| {
        s.iter()
            .filter(|((d, _), _)| *d == dim)
            .map(|((_, p), s)| (*p, *s))
            .collect()
    });
    for (pos, s) in seats {
        let stand = lv.host.entity(s.id);
        let ridden = stand.is_some_and(|e| !e.passengers(&*lv.host).unwrap_or_default().is_empty());
        if is_seat(lv, pos) && ridden {
            continue;
        }
        // The seat lets its riders off where Create's do (`getDismountLocationForPassenger`)
        // and goes.
        if let Some(e) = stand {
            e.teleport_to(&mut *lv.host, seat::dismount_at(pos), None);
            e.discard(&mut *lv.host);
        }
        SEATS.with_borrow_mut(|m| m.remove(&(dim, pos)));
    }
}

/// `SeatBlock.updateEntityAfterFallOn`: a living mob that lands on a free seat, standing in its
/// block, sits down.
fn seat_inside(lv: &mut ServerLevel<'_>, pos: [i32; 3], e: EntityRef) {
    let kind = kind_of(lv, e);
    if e.vehicle(&*lv.host).is_some() || kind == "minecraft:player" {
        return;
    }
    let occupied = SEATS.with_borrow(|s| s.contains_key(&(lv.dim, pos)));
    let feet = e.position(&*lv.host).unwrap_or_default();
    let on_ground = e.on_ground(&*lv.host).unwrap_or(false);
    if !seat::sits_down(pos, &kind, feet, on_ground, false, occupied) || lv.host.mob(e).is_none() {
        return;
    }
    sit_down(lv, pos, e);
}

/// `TurntableBlock.entityInside` on a turntable turning at its speed: a mob standing on it
/// turns by 3/10 of the speed a tick (its head with it); a player standing on it (their client
/// turns them, `turn_local_player`) whose middle is off it is carried round it
/// (`setDeltaMovement` on the server's copy of their motion).
fn turntable_inside(lv: &mut ServerLevel<'_>, pos: [i32; 3], e: EntityRef) {
    let dim = lv.dim;
    let rpm = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
    let speed = turntable::mob_turn(rpm);
    if speed == 0.0 {
        return;
    }
    let host = &*lv.host;
    let (Some(feet), Some(v), Some([yaw, pitch])) =
        (e.position(host), e.velocity(host), e.rotation(host))
    else {
        return;
    };
    let on_ground = e.on_ground(host).unwrap_or(false);
    let stands = turntable::stands_on(pos, feet, on_ground, v[1]);
    if kind_of(lv, e) != "minecraft:player" {
        if stands {
            e.set_rotation(&mut *lv.host, yaw - speed, pitch);
            e.set_head_yaw(&mut *lv.host, yaw - speed);
        }
        return;
    }
    let middle = feet.map(|v| v.floor() as i32);
    if stands && lv.block_state(middle) != lv.block_state(pos) {
        let m = turntable::carry(pos, feet, speed);
        e.set_velocity(&mut *lv.host, [v[0] + m[0], v[1] + m[1], v[2] + m[2]]);
    }
}

/// `TurntableHandler`: the local player standing on a turning turntable (`getOnPos`) turns
/// with it, on their own client.
fn turn_local_player(host: &mut dyn ClientHost) {
    let Some(player) = host.local_player() else {
        return;
    };
    let e = player.entity();
    let (Some(feet), Some(level), Some([yaw, pitch])) =
        (e.position(&*host), player.level(&*host), e.rotation(&*host))
    else {
        return;
    };
    let below = [feet[0], feet[1] - 1.0e-5, feet[2]].map(|v| v.floor() as i32);
    let Some(state) = level.block_state(&*host, below) else {
        return;
    };
    // The client's own state table: the server's world isn't on this thread.
    let turntable = super::super::states::id(state).and_then(|id| {
        super::super::client_table(&*host, |t| matches!(t.get(id), Some((Block::Turntable, _))))
    });
    if turntable != Some(true) {
        return;
    }
    let rpm = host
        .block_entity(level, below)
        .and_then(|b| b.data(&*host))
        .map_or(0.0, |d| match d.get(&"Speeds".into()) {
            Some(mod_api::Tag::List(l)) => match l.first() {
                Some(mod_api::Tag::Float(bits)) => f32::from_bits(*bits),
                _ => 0.0,
            },
            _ => 0.0,
        });
    let turn = turntable::player_turn(rpm, below, feet);
    if turn != 0.0 {
        let _ = host.set_local_rotation(yaw - turn, pitch);
    }
}

struct SeatPart;

#[mod_api::sunrise_part]
impl BlockPart for SeatPart {
    /// `SeatBlock.useItemOn` on the server: sneaking passes; a dye of another colour recolours
    /// the seat (`BlockHelper.copyProperties`); a seat a player sits on passes; otherwise the
    /// clicking player sits down (taking the seat from a mob on it), at the level's next tick.
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let c = clicker(&*host, event.click);
        let pos = event.click.pos;
        let player = event.click.player;
        in_click_level(host, event.click, |lv| {
            let block = lv.create_block(pos).filter(|b| dye::is_seat(*b))?;
            if c.sneaking {
                return None;
            }
            if let Some(to) = c
                .held
                .as_deref()
                .and_then(DyeColor::of_item)
                .and_then(|colour| dye::recoloured(block, colour))
            {
                let (_, props) = lv.name_props(pos)?;
                let p: Vec<(&str, &str)> = props
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                if let Some(id) = super::state_with(&*lv.host, &format!("create:{}", to.name()), &p)
                {
                    lv.set_num(pos, id, update_all());
                }
                return Some(InteractionResult::Success);
            }
            let dim = lv.dim;
            if SEATS.with_borrow(|s| s.get(&(dim, pos)).is_some_and(|s| s.player)) {
                return None;
            }
            SITS.with_borrow_mut(|s| s.push((dim, pos, player)));
            Some(InteractionResult::Success)
        })?
    }

    /// `SeatBlock.updateEntityAfterFallOn`: a player, an entity that can't sit (a shulker, a
    /// thing not living) or a mob landing on a taken seat bounces off at 0.66 of its speed
    /// (the game takes 0.8 of that for one not living); a mob that will sit doesn't bounce.
    fn bounce_restitution(
        &self,
        _level: mod_api::LevelSide<'_>,
        event: &mut mod_api::BounceRestitutionEvent<'_>,
    ) {
        // The part is on the seats and the turntable.
        if event.state.block() == key(Block::Turntable) {
            return;
        }
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        let taken = SEATS.with_borrow(|s| s.keys().any(|(_, p)| *p == pos));
        if !seat::can_sit(event.kind) || taken {
            event.factor = event.factor.max(0.66);
        }
    }

    /// `SeatBlock.fallOn`: the fall counted at half its distance (the turntable's is whole).
    fn fall_on(&self, _level: &mut dyn LevelRandomHost, event: &mut FallOnEvent<'_>) {
        if event.state.block() != key(Block::Turntable) {
            event.distance = seat::fall_distance(event.distance);
        }
    }

    /// `BlockBehaviour.entityInside` for seats and turntables: the entity, for the level's tick.
    fn entity_inside(&self, _level: &mut dyn LevelRandomHost, event: &mut EntityInsideEvent<'_>) {
        let Some(dim) = level_index(event.dimension) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        INSIDE.with_borrow_mut(|i| i.push((dim, pos, event.entity as u32)));
    }
}
