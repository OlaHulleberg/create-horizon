//! The encased fan (`EncasedFanBlockEntity`, `AirCurrent`): its current pushes entities and
//! processes items in it (`FanProcessing`: washing, haunting, smoking, blasting), on the
//! ground and on depots and belts in it; and the nozzle on its front (`NozzleBlockEntity`).
//! The processing types' effects on the other entities in a current (`affectEntity`): blasting
//! and smoking set them alight and hurt them, washing puts them out and hurts endermen, snow
//! golems and blazes, haunting blinds and slows them. Create's own damage types (`fan_lava`,
//! `fan_fire`) are stood in for by vanilla's `lava` and `in_fire` (`API_GAPS.md`: a native
//! mod's damage types). A horse haunted for 100 ticks turns into a skeleton horse made from its
//! saved data (`Haunted`, its progress kept on the horse as Create's `CreateHaunting`).
//! The processing particles of the items in a current (`spawnProcessingParticles`) are sent from
//! here; the clients get the currents to push their own players and draw the air (`super::air`).

use super::{recipes, stacks};
use crate::glue::logistics::{random_angle, upright};
use crate::glue::net;
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::fan::{self, Catalyst, Current, Heat, Kind, Way};
use create_engine::kinetics::{Axis, Dir};
use create_engine::{Block, belt, depot, nozzle, processing};
use mod_api::{
    AttachmentCodec, AttachmentError, AttachmentType, BlockShapeKind, Compound, EntityPart,
    EntityPartState, EntityRef, ItemStack, ParticleOptions, PlayerRef, SoundSource, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// What a level's clients were last told of its fans: the message, and the players told.
type Told = (Vec<u8>, Vec<u128>);

thread_local! {
    /// Per level: the turning fans' currents last told its clients, and to whom.
    static TOLD: RefCell<HashMap<usize, Told>> = RefCell::new(HashMap::new());
    /// The players last told they fly in creative.
    static FLYING: RefCell<Vec<u128>> = const { RefCell::new(Vec::new()) };
    static PARTICLE_RNG: RefCell<processing::Rng> = RefCell::new(processing::Rng::new(0xFA2));
}

/// A player (back) in the world: their client knows no fans yet.
pub(super) fn player_joined(player: PlayerRef) {
    let uuid = player.uuid();
    TOLD.with_borrow_mut(|t| t.values_mut().for_each(|(_, to)| to.retain(|u| *u != uuid)));
    FLYING.with_borrow_mut(|f| f.retain(|u| *u != uuid));
}

/// The end of a player's server tick: their client is told when they start or stop flying in
/// creative (`isPlayerCreativeFlying`), which lets go of them in a current.
pub(super) fn player_tick(host: &mut dyn mod_api::ServerHost, player: PlayerRef) {
    let flying = player
        .abilities(&*host)
        .is_some_and(|a| a.flying && a.instabuild);
    let uuid = player.uuid();
    let told = FLYING.with_borrow(|f| f.contains(&uuid));
    if flying == told {
        return;
    }
    FLYING.with_borrow_mut(|f| {
        if flying {
            f.push(uuid);
        } else {
            f.retain(|u| *u != uuid);
        }
    });
    net::send_to_player(
        host,
        player,
        super::air::FLYING_CHANNEL,
        &[u8::from(flying)],
    );
}

/// The level's turning fans and their currents (`AirCurrent`, which Create keeps on both sides
/// in its block entity), told to its players when they change and to a player new to the
/// level: their clients push their own players and draw the air flow (`super::air`).
fn tell_currents(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let seen = with_world(|w| {
        let mut fans: Vec<fan::Seen> = w.dims[dim]
            .fans
            .iter()
            .filter(|(_, f)| f.speed != 0.0 && f.current.max > 0.0)
            .filter_map(|(&pos, f)| {
                Some(fan::Seen {
                    pos,
                    facing: f.facing?,
                    speed: f.speed,
                    current: f.current.clone(),
                })
            })
            .collect();
        fans.sort_by_key(|f| f.pos);
        fans
    })
    .unwrap_or_default();
    let data = fan::encode_seen(&seen);
    let players = lv.players();
    let to: Vec<PlayerRef> = TOLD.with_borrow_mut(|t| {
        let (told, had) = t.entry(dim).or_default();
        let changed = *told != data;
        let to = players
            .iter()
            .filter(|p| changed || !had.contains(&p.uuid()))
            .copied()
            .collect();
        *told = data.clone();
        *had = players.iter().map(|p| p.uuid()).collect();
        to
    });
    if !to.is_empty() {
        net::send_to_players(&mut *lv.host, &to, super::air::CHANNEL, &data);
    }
}

fn particle_random() -> f32 {
    PARTICLE_RNG.with_borrow_mut(processing::Rng::next_float)
}

/// `FanProcessingType.spawnProcessingParticles` for an item at `at` being processed by `kind`
/// (Create spawns them on the client, which sees the same items; here the server sends them).
fn processing_particles(lv: &mut ServerLevel<'_>, kind: Kind, at: [f64; 3]) {
    let Some(keys) = super::air::KEYS.get() else {
        return;
    };
    if particle_random() * 8.0 >= 1.0 {
        return;
    }
    let send = |lv: &mut ServerLevel<'_>, options: ParticleOptions, at: [f64; 3], v| {
        let _ = lv
            .level
            .send_particles(&mut *lv.host, &options, None, at, v, 1.0, 0);
    };
    let [x, y, z] = at;
    match kind {
        Kind::Blasting => send(
            lv,
            ParticleOptions::simple(keys.large_smoke),
            [x, y + 0.25, z],
            [0.0, 1.0 / 16.0, 0.0],
        ),
        Kind::Haunting => {
            // `offsetRandomly(ZERO, random, 1).multiply(1, .05, 1).normalize().scale(.15)`.
            let o = [1.0, 0.05, 1.0].map(|m| f64::from((particle_random() - 0.5) * 2.0) * m);
            let len = o.iter().map(|v| v * v).sum::<f64>().sqrt();
            let o = if len < 1e-4 {
                [0.0; 3]
            } else {
                o.map(|v| v / len * 0.15)
            };
            let [x, y, z] = [x + o[0], y + o[1], z + o[2]];
            send(
                lv,
                ParticleOptions::simple(keys.soul_fire_flame),
                [x, y + 0.45, z],
                [0.0; 3],
            );
            if particle_random() < 0.5 {
                send(
                    lv,
                    ParticleOptions::simple(keys.smoke),
                    [x, y + 0.25, z],
                    [0.0; 3],
                );
            }
        }
        Kind::Smoking => send(
            lv,
            ParticleOptions::simple(keys.poof),
            [x, y + 0.25, z],
            [0.0, 1.0 / 16.0, 0.0],
        ),
        Kind::Splashing => {
            let spread = || f64::from((particle_random() - 0.5) * 0.5);
            let dust = ParticleOptions::dust(keys.dust, [0.0, 0x55 as f32 / 255.0, 1.0], 1.0);
            let p = [x + spread(), y + 0.5, z + spread()];
            send(lv, dust, p, [0.0, 1.0 / 8.0, 0.0]);
            let p = [x + spread(), y + 0.5, z + spread()];
            send(
                lv,
                ParticleOptions::simple(keys.spit),
                p,
                [0.0, 1.0 / 8.0, 0.0],
            );
        }
    }
}

/// Encased fans that turn (`EncasedFanBlockEntity.tick`): the current looked at again when due
/// (`AirCurrent.rebuild`), then the depots and belts in it processed (`tickAffectedHandlers`).
/// Stopped fans cost nothing.
pub fn tick_fans(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    check_nozzles(lv);
    let fans = with_world(|w| w.fans_to_tick(dim)).unwrap_or_default();
    if fans.is_empty() {
        tell_currents(lv);
        return;
    }
    let recipes = recipes::ready();
    // Item entities' progress, kept for those still in a current this tick.
    let mut kept = HashMap::new();
    let mut old = with_world(|w| std::mem::take(&mut w.dims[dim].item_fan)).unwrap_or_default();
    for pos in fans {
        let Some((rebuild, speed, facing)) = with_world(|w| {
            let f = w.dims[dim].fans.get(&pos)?;
            Some((f.rebuild, f.speed, f.facing))
        })
        .flatten() else {
            continue;
        };
        if rebuild {
            let (current, handlers) = match facing {
                Some(facing) if speed != 0.0 => fan_current(lv, pos, facing, speed),
                _ => Default::default(),
            };
            if current.max > 0.0 {
                crate::glue::advancements::award_placer(lv, pos, "encased_fan");
            }
            with_world(|w| {
                if let Some(f) = w.dims[dim].fans.get_mut(&pos) {
                    f.rebuild = false;
                    f.changed |= f.current != current;
                    f.current = current;
                    f.handlers = handlers;
                }
            });
        }
        // The nozzle on its front (`NozzleBlockEntity.tick`): it does something only while its
        // fan turns, so it ticks with it (and once more when it stops, to let go).
        if let Some(f) = facing {
            tick_nozzle(lv, super::step(pos, f, 1), (speed, f));
        }
        if speed == 0.0 {
            continue;
        }
        // `entitySearchCooldown`: the entities in the current are looked for every 6th tick.
        let search = with_world(|w| {
            let f = w.dims[dim].fans.get_mut(&pos)?;
            let search = f.search_cooldown == 0;
            f.search_cooldown = if search { 5 } else { f.search_cooldown - 1 };
            Some(search)
        })
        .flatten()
        .unwrap_or(false);
        fan_on_entities(lv, pos, search);
        if !recipes {
            continue;
        }
        fan_on_items(lv, pos, search, &mut old, &mut kept);
        let handlers = with_world(|w| w.dims[dim].fans.get(&pos).map(|f| f.handlers.clone()))
            .flatten()
            .unwrap_or_default();
        for (at, kind) in handlers {
            let Some(kind) = kind else {
                continue;
            };
            if fan_on_depot(lv, at, kind) | fan_on_belt(lv, at, kind) {
                crate::glue::advancements::award_placer(lv, pos, "fan_processing");
            }
        }
    }
    with_world(|w| w.dims[dim].item_fan = kept);
    tell_currents(lv);
}

/// Nozzles whose neighbour changed break, with their drops, when the block behind them is no
/// longer an encased fan facing their way (`NozzleBlock.neighborChanged`, `canSurvive`).
fn check_nozzles(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let checks = with_world(|w| {
        let c = &mut w.dims[dim].nozzle_checks;
        if c.is_empty() {
            Vec::new()
        } else {
            std::mem::take(c)
        }
    })
    .unwrap_or_default();
    for pos in checks {
        let Some(facing) = lv.facing_of(pos, Block::Nozzle) else {
            continue;
        };
        let fan = lv.facing_of(super::step(pos, facing, -1), Block::EncasedFan);
        if !nozzle::survives(facing, fan) {
            lv.destroy_block(pos, true);
        }
    }
}

/// An entity that is not an item, as the fan and the nozzle push it.
struct Pushed {
    entity: EntityRef,
    at: [f64; 3],
    velocity: [f64; 3],
    min: [f64; 3],
    max: [f64; 3],
    sneaking: bool,
    /// `isPlayerCreativeFlying` (and a spectator's flight).
    flying: bool,
    riding: bool,
}

/// The entities but items in `lo`..`hi`.
fn others_in(lv: &mut ServerLevel<'_>, lo: [f64; 3], hi: [f64; 3]) -> Vec<Pushed> {
    let found = lv.level.entities_in(&*lv.host, lo, hi).unwrap_or_default();
    let mut out = Vec::new();
    for entity in found {
        if lv.host.item_entity(entity).is_some() {
            continue;
        }
        let flying = if entity.kind(&*lv.host).map(|k| k.id(&*lv.host)).as_deref()
            == Some("minecraft:player")
        {
            entity
                .uuid(&mut *lv.host)
                .and_then(|u| lv.host.player(u))
                .and_then(|p| p.abilities(&*lv.host))
                .is_some_and(|a| a.flying && a.instabuild)
        } else {
            false
        };
        let host = &*lv.host;
        let (Some([min, max]), Some(at)) = (entity.bounding_box(host), entity.position(host))
        else {
            continue;
        };
        out.push(Pushed {
            entity,
            at,
            velocity: entity.velocity(host).unwrap_or_default(),
            min,
            max,
            sneaking: entity.is_shift_key_down(host).unwrap_or(false),
            flying,
            riding: entity.vehicle(host).is_some(),
        });
    }
    out
}

/// `NozzleBlockEntity.tick` for the nozzle at `pos` on the front of a ticking fan (`fan`: its
/// speed and facing): every sixth tick it looks for the entities within half its range that it
/// sees (`lazyTick`, `canSee` through whole blocks); then it takes its range from the fan and
/// pushes (or pulls) each caught one by `(range - distance)` along the line from its centre,
/// 1/128 for items and 1/32 for anything else a tick, forgetting its fall (`fallDistance = 0`).
/// Sneaking players and flyers are let go. Pulling more than 256 discards the items and mobs it
/// holds, after a power 2 explosion that breaks no block (`Level.explode`).
fn tick_nozzle(lv: &mut ServerLevel<'_>, pos: [i32; 3], fan: (f32, Dir)) {
    use nozzle::{Push, Target};
    let dim = lv.dim;
    let Some(mut n) = with_world(|w| w.dims[dim].nozzles.remove(&pos)).flatten() else {
        return;
    };
    if n.facing != fan.1 {
        with_world(|w| w.dims[dim].nozzles.insert(pos, n));
        return;
    }
    let reach = |range: f32| {
        let c = nozzle::center(pos);
        let r = f64::from(range);
        (c.map(|v| v - r), c.map(|v| v + r))
    };
    if n.lazy_due() && n.range != 0.0 {
        let (lo, hi) = nozzle::search_box(pos, n.range);
        let mut seen: Vec<(Target, [f64; 3])> = Vec::new();
        let (rlo, rhi) = reach(n.range);
        let mut alive: Vec<Target> = Vec::new();
        let items = lv.items_in(rlo, rhi);
        for d in &items {
            alive.push(Target::Item(d.id()));
            if belt::item_touches(d.at, lo, hi) {
                seen.push((Target::Item(d.id()), d.at));
            }
        }
        let others = others_in(lv, rlo, rhi);
        for e in &others {
            let t = Target::Entity(e.entity.id());
            alive.push(t);
            // Sneaking players and flyers are skipped.
            if e.sneaking || e.flying {
                continue;
            }
            if (0..3).all(|a| e.min[a] < hi[a] && e.max[a] > lo[a]) {
                seen.push((t, e.at));
            }
        }
        for (t, at) in seen {
            n.look_at(pos, t, at, || {
                nozzle::sees(pos, at, |p| p != pos && lv.blocked(p))
            });
        }
        n.entities.retain(|t| alive.contains(t));
        if nozzle::overloaded(n.pushing, n.entities.len()) {
            // `level.explode(.., 2, ExplosionInteraction.NONE)`, then what it held vanishes.
            lv.explode_sparing_blocks(nozzle::center(pos), 2.0);
            let gone = std::mem::take(&mut n.entities);
            for d in &items {
                if gone.contains(&Target::Item(d.id())) {
                    lv.set_dropped(d, None, d.velocity);
                }
            }
            for e in &others {
                if gone.contains(&Target::Entity(e.entity.id())) {
                    e.entity.discard(&mut *lv.host);
                }
            }
        }
    }
    n.update_range(Some(fan));
    if !n.entities.is_empty() {
        let (range, pushing) = (n.range, n.pushing);
        let (lo, hi) = reach(range);
        let mut released: Vec<Target> = Vec::new();
        let mut found: Vec<Target> = Vec::new();
        for d in lv.items_in(lo, hi) {
            let t = Target::Item(d.id());
            if !n.entities.contains(&t) {
                continue;
            }
            found.push(t);
            match nozzle::push(pos, range, pushing, d.at, true, false) {
                Push::Release => released.push(t),
                Push::Hold => {}
                Push::Add(add) => {
                    let v = d.velocity;
                    let motion = [v[0] + add[0], v[1] + add[1], v[2] + add[2]];
                    lv.set_dropped(&d, Some(d.stack.clone()), motion);
                }
            }
        }
        if n.entities.iter().any(|t| !t.is_item()) {
            for e in others_in(lv, lo, hi) {
                let t = Target::Entity(e.entity.id());
                if !n.entities.contains(&t) {
                    continue;
                }
                found.push(t);
                match nozzle::push(pos, range, pushing, e.at, false, e.sneaking || e.flying) {
                    Push::Release => released.push(t),
                    Push::Hold => {}
                    Push::Add(add) => {
                        let v = e.velocity;
                        e.entity.set_velocity(
                            &mut *lv.host,
                            [v[0] + add[0], v[1] + add[1], v[2] + add[2]],
                        );
                        e.entity.reset_fall_distance(&mut *lv.host);
                    }
                }
            }
        }
        // Gone, or out of the box and so out of range: let go.
        n.entities
            .retain(|t| found.contains(t) && !released.contains(t));
    }
    with_world(|w| w.dims[dim].nozzles.insert(pos, n));
}

/// `AirCurrent.tickAffectedEntities` for items: those its search caught (every 6th tick) and
/// still in its box are pushed along the current, and processed by the type at their distance
/// (`FanProcessing.applyProcessing`): the first result replaces the stack, the others are
/// dropped beside it with its motion, none discards it.
fn fan_on_items(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    search: bool,
    old: &mut HashMap<u32, fan::Progress>,
    kept: &mut HashMap<u32, fan::Progress>,
) {
    let dim = lv.dim;
    let Some((facing, speed, current)) = with_world(|w| {
        let f = w.dims[dim].fans.get(&pos)?;
        Some((f.facing?, f.speed, f.current.clone()))
    })
    .flatten() else {
        return;
    };
    let Some((lo, hi)) = fan::bounds(pos, facing, current.max) else {
        return;
    };
    let items = lv.items_in(lo, hi);
    let caught: Vec<u32> = if search {
        let ids: Vec<u32> = items.iter().map(|i| i.id()).collect();
        with_world(|w| {
            if let Some(f) = w.dims[dim].fans.get_mut(&pos) {
                f.caught = ids.clone();
            }
        });
        ids
    } else {
        with_world(|w| w.dims[dim].fans.get(&pos).map(|f| f.caught.clone()))
            .flatten()
            .unwrap_or_default()
    };
    if items.is_empty() {
        return;
    }
    let flow = if current.pushing {
        facing
    } else {
        facing.opposite()
    };
    let centre = pos.map(|v| f64::from(v) + 0.5);
    let mut rng =
        processing::Rng::new(lv.game_time() as u64 ^ (pos[0] as u64) << 16 ^ pos[2] as u64);
    for d in items {
        if !caught.contains(&d.id()) {
            continue;
        }
        let distance = (0..3)
            .map(|a| (d.at[a] - centre[a]).powi(2))
            .sum::<f64>()
            .sqrt();
        let motion = fan::push(flow, speed, current.max, distance, d.velocity, false);
        let mut next = Some(d.stack.clone());
        if let Some(kind) = current.type_at(fan::aligned_distance(d.at, pos, facing) as f32) {
            processing_particles(lv, kind, d.at);
            let id = d.id();
            let mut p = kept
                .remove(&id)
                .or_else(|| old.remove(&id))
                .unwrap_or_default();
            let host = &*lv.host;
            let item = stacks::id(host, &d.stack);
            let fire_resistant = fire_resistant(lv, &d.stack);
            let count = d.stack.count().min(255) as u8;
            let max = |i: &str| stacks::limit(host, i);
            let done = recipes::with(|r| {
                let can = fan::can_process(kind, &item, fire_resistant, r.fan());
                if !fan::entity_can(&p, kind, can)
                    || !fan::advance_entity(&mut p, kind, u32::from(count))
                {
                    return None;
                }
                fan::process(kind, &item, count, r.fan(), max, &mut rng)
            })
            .flatten();
            kept.insert(id, p);
            if done.is_some() {
                crate::glue::advancements::award_placer(lv, pos, "fan_processing");
            }
            if let Some(mut out) = done {
                if out.is_empty() {
                    next = None;
                } else {
                    let (first, n) = out.remove(0);
                    for (i, c) in out {
                        lv.spawn_moving(d.at, &i, u32::from(c), motion);
                    }
                    next = stacks::stack(&*lv.host, &first, u32::from(n));
                }
            }
        }
        lv.set_dropped(&d, next, motion);
    }
}

/// Whether a stack's item resists fire (`DataComponents.DAMAGE_RESISTANT` with `is_fire`).
fn fire_resistant(lv: &ServerLevel<'_>, stack: &ItemStack) -> bool {
    lv.host
        .item_default_component(stack.item(), "minecraft:damage_resistant")
        .is_some()
}

/// `AirCurrent.tickAffectedEntities` for every entity but items (mobs, players, minecarts,
/// boats, armor stands): those its search caught (`search`, every 6th tick) and still in its box
/// are pushed along the current as items are (`setDeltaMovement`) and forget their fall
/// (`fallDistance = 0`). A rider moves with its vehicle (`rideTick` zeroes its motion), so it is
/// left alone. A player's motion is its client's (`super::air` pushes it there, as Create's
/// client does); here a player only forgets its fall. A flying creative player is let go
/// (`isPlayerCreativeFlying`).
fn fan_on_entities(lv: &mut ServerLevel<'_>, pos: [i32; 3], search: bool) {
    let dim = lv.dim;
    let Some((facing, speed, current, none)) = with_world(|w| {
        let f = w.dims[dim].fans.get(&pos)?;
        Some((f.facing?, f.speed, f.current.clone(), f.others.is_empty()))
    })
    .flatten() else {
        return;
    };
    let (max, pushing) = (current.max, current.pushing);
    // Nothing caught and no search this tick: nothing to look at.
    if none && !search {
        return;
    }
    let Some((lo, hi)) = fan::bounds(pos, facing, max) else {
        return;
    };
    let inside = others_in(lv, lo, hi);
    let others: Vec<u32> = if search {
        let ids: Vec<u32> = inside.iter().map(|e| e.entity.id()).collect();
        with_world(|w| {
            if let Some(f) = w.dims[dim].fans.get_mut(&pos) {
                f.others = ids.clone();
            }
        });
        ids
    } else {
        with_world(|w| w.dims[dim].fans.get(&pos).map(|f| f.others.clone()))
            .flatten()
            .unwrap_or_default()
    };
    let flow = if pushing { facing } else { facing.opposite() };
    let centre = pos.map(|v| f64::from(v) + 0.5);
    for e in inside {
        let id = e.entity.id();
        if !others.contains(&id) || e.riding {
            continue;
        }
        // `isPlayerCreativeFlying`: let go until the next search.
        if e.flying {
            with_world(|w| {
                if let Some(f) = w.dims[dim].fans.get_mut(&pos) {
                    f.others.retain(|o| *o != id);
                }
            });
            continue;
        }
        let distance = (0..3)
            .map(|a| (e.at[a] - centre[a]).powi(2))
            .sum::<f64>()
            .sqrt();
        let motion = fan::push(flow, speed, max, distance, e.velocity, e.sneaking);
        e.entity.set_velocity(&mut *lv.host, motion);
        e.entity.reset_fall_distance(&mut *lv.host);
        // `getTypeAt(alignedDistanceToFace)`, then `affectEntity`.
        if let Some(kind) = current.type_at(fan::aligned_distance(e.at, pos, facing) as f32) {
            affect_entity(lv, e.entity, kind);
        }
    }
}

/// `FanProcessingType.affectEntity` on the server, for an entity (not an item) in a current.
fn affect_entity(lv: &mut ServerLevel<'_>, entity: EntityRef, kind: Kind) {
    let host = &mut *lv.host;
    // `igniteForSeconds`: at least that many ticks of fire.
    let ignite = |host: &mut dyn mod_api::ServerHost, ticks: i32| {
        if entity.fire_ticks(&*host).unwrap_or(0) < ticks {
            entity.set_remaining_fire_ticks(host, ticks);
        }
    };
    match kind {
        // `BlastingType`: 10 s of fire and 4 lava damage (`fanLava`), unless fire-immune.
        Kind::Blasting => {
            if entity.fire_immune(&*host) != Some(true) {
                ignite(host, 200);
                let _ = entity.hurt(host, "minecraft:lava", 4.0, None, None);
            }
        }
        // `SmokingType`: 2 s of fire and 2 fire damage (`fanFire`), unless fire-immune.
        Kind::Smoking => {
            if entity.fire_immune(&*host) != Some(true) {
                ignite(host, 40);
                let _ = entity.hurt(host, "minecraft:in_fire", 2.0, None, None);
            }
        }
        // `SplashingType`: drowning hurts endermen, snow golems and blazes; a burning entity is
        // put out with the extinguish sound (0.7, 1.6 ± 0.4: `Entity.extinguishFire`'s).
        Kind::Splashing => {
            let kind = entity.kind(&*host).map(|k| k.id(&*host));
            if matches!(
                kind.as_deref(),
                Some("minecraft:enderman" | "minecraft:snow_golem" | "minecraft:blaze")
            ) {
                let _ = entity.hurt(host, "minecraft:drown", 2.0, None, None);
            }
            if entity.fire_ticks(&*host).unwrap_or(0) > 0 {
                entity.extinguish_fire(host);
            }
        }
        // `HauntingType`: Blindness for 30 ticks and Slowness II for 20, without particles or
        // icons (`MobEffectInstance(.., false, false)`).
        Kind::Haunting => {
            let Some(living) = host.living(entity) else {
                return;
            };
            for (effect, ticks, amplifier) in [
                ("minecraft:blindness", 30, 0),
                ("minecraft:slowness", 20, 1),
            ] {
                if let Some(key) = host.effect_key(effect) {
                    living.add_effect(
                        host,
                        &mod_api::ActiveEffect::new(key, ticks, amplifier, false, false, false),
                    );
                }
            }
            haunt_horse(lv, entity);
        }
    }
}

/// A horse's haunting progress (`getPersistentData().getInt("CreateHaunting")`), saved with it.
static HAUNTING: AttachmentType<u32> = AttachmentType {
    namespace: "create",
    name: "haunting",
    default: || 0,
    codec: Some(AttachmentCodec {
        encode: |n| Ok([("CreateHaunting".into(), Tag::Int(*n as i32))].into()),
        decode: |saved: &Compound| match saved.get(&"CreateHaunting".into()) {
            Some(Tag::Int(n)) => Ok((*n).max(0) as u32),
            _ => Err(AttachmentError::Decode),
        },
    }),
    copy_on_death: false,
    sync: false,
};

/// Keeps the haunting progress on each horse (`HauntingType.affectEntity`'s `Horse`).
pub struct Haunted;
#[mod_api::horizon_part]
impl EntityPart for Haunted {}
impl EntityPartState for Haunted {
    type Data = u32;
    const DATA: &'static AttachmentType<u32> = &HAUNTING;
}

/// `HauntingType.affectEntity` for a horse: 100 ticks in a haunting current, with the soul
/// escape sound every 10th rising in pitch, then the extinguish sound and a skeleton horse from
/// the horse's saved data (its UUID left out) where it stood, its armour dropped, the horse gone.
fn haunt_horse(lv: &mut ServerLevel<'_>, entity: EntityRef) {
    let host = &mut *lv.host;
    if entity.kind(&*host).map(|k| k.id(&*host)).as_deref() != Some("minecraft:horse") {
        return;
    }
    let Some(at) = entity.position(&*host) else {
        return;
    };
    let block = at.map(|v| v.floor() + 0.5);
    let Ok(progress) = Haunted::edit_state(host, entity, |_, n| {
        let was = *n;
        if was < 100 {
            *n = was + 1;
        }
        was
    }) else {
        return;
    };
    if progress < 100 {
        if progress % 10 == 0 {
            let pitch = 1.5 * progress as f32 / 100.0;
            let _ = lv.level.play_sound(
                &mut *lv.host,
                "minecraft:particle.soul_escape",
                block,
                SoundSource::Neutral,
                1.0,
                pitch,
            );
        }
        return;
    }
    let _ = lv.level.play_sound(
        &mut *lv.host,
        "minecraft:block.fire.extinguish",
        block,
        SoundSource::Neutral,
        1.25,
        0.65,
    );
    let host = &mut *lv.host;
    let Some(mut saved) = entity.nbt(host) else {
        return;
    };
    saved.remove(&"UUID".into());
    // `getBodyArmorItem`: the horse's body slot (`equipment.body`), dropped where it stood.
    let armour = match saved.get(&"equipment".into()) {
        Some(Tag::Compound(e)) => match e.get(&"body".into()) {
            Some(Tag::Compound(body)) => ItemStack::from_nbt(&*host, body),
            _ => None,
        },
        _ => None,
    };
    if let Some(armour) = armour {
        let _ = lv
            .level
            .spawn_item(&mut *lv.host, at[0], at[1], at[2], armour, 10);
    }
    // `EntityType.SKELETON_HORSE.create`, then `deserializeNBT` of the horse's data.
    let spawned = lv
        .host
        .summon(
            lv.dimension,
            "minecraft:skeleton_horse",
            at,
            Compound::new(),
        )
        .and_then(|id| lv.host.entity(id));
    if let Some(s) = spawned {
        s.merge_nbt(&mut *lv.host, saved);
    }
    entity.discard(&mut *lv.host);
}

/// The depots and belts in a current, with the processing type each is in.
type Handlers = Vec<([i32; 3], Option<Kind>)>;

/// `AirCurrent.getFlowLimit` from `pos` along `facing`, up to `max` blocks.
pub(crate) fn flow_limit_from(lv: &ServerLevel<'_>, pos: [i32; 3], facing: Dir, max: f32) -> f32 {
    let way = |i: i32| {
        let p = super::step(pos, facing, i);
        if !lv.is_loaded(p) {
            return Way::Unloaded;
        }
        if lv.block_tagged(p, "create:fan_transparent") {
            return Way::Open;
        }
        let shape = lv.shape(p, BlockShapeKind::Collision);
        if shape.is_empty() {
            Way::Open
        } else if shape.len() == 1 && shape[0] == [0.0, 0.0, 0.0, 1.0, 1.0, 1.0] {
            Way::Full
        } else {
            Way::Depth(max_depth(&shape, facing))
        }
    };
    fan::flow_limit(max, way)
}

/// `AirCurrent.rebuild` and `findAffectedHandlers` for the fan at `pos`.
fn fan_current(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    facing: Dir,
    speed: f32,
) -> (Current, Handlers) {
    let dim = lv.dim;
    let at = |i: i32| super::step(pos, facing, i);
    let max = flow_limit_from(lv, pos, facing, fan::max_distance(speed));
    let current = Current::build(max, fan::pushing(speed, facing), |i| catalyst_at(lv, at(i)));
    let mut handlers = Vec::new();
    for i in 1..=current.limit() {
        let segment = current.type_at((i - 1) as f32);
        for below in 0..2 {
            let p = at(i);
            let p = [p[0], p[1] - below, p[2]];
            if super::takes_from_side(dim, p) {
                handlers.push((p, catalyst_at(lv, p).or(segment)));
            }
            if facing.axis() == Axis::Y {
                break;
            }
        }
    }
    (current, handlers)
}

/// `AirCurrent.findMaxDepth`: how far air gets into a shape (its boxes) going `facing`,
/// sampled at five points of the face; infinite where a sample finds a hole.
fn max_depth(shape: &[[f64; 6]], facing: Dir) -> f32 {
    let axis = match facing.axis() {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    };
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let positive = facing.positive();
    let mut depth = 0.0f64;
    for (a, b) in [
        (0.25, 0.25),
        (0.25, 0.75),
        (0.5, 0.5),
        (0.75, 0.25),
        (0.75, 0.75),
    ] {
        let hit = shape
            .iter()
            .filter(|s| s[u] <= a && a <= s[u + 3] && s[v] <= b && b <= s[v + 3]);
        let d = if positive {
            let min = hit.map(|s| s[axis]).fold(f64::INFINITY, f64::min);
            if min == f64::INFINITY {
                return f32::INFINITY;
            }
            min
        } else {
            let max = hit.map(|s| s[axis + 3]).fold(f64::NEG_INFINITY, f64::max);
            if max == f64::NEG_INFINITY {
                return f32::INFINITY;
            }
            1.0 - max
        };
        depth = depth.max(d);
    }
    depth as f32
}

/// `FanProcessingType.getAt` at a block: its fluid and the catalyst block tags with the state
/// properties they check (a campfire's `lit`, a blaze burner's `blaze` and `flame_type`).
fn catalyst_at(lv: &ServerLevel<'_>, p: [i32; 3]) -> Option<Kind> {
    let fluid = lv
        .level
        .fluid_state(&*lv.host, p)
        .filter(|f| f.amount > 0)
        .map(|f| f.fluid.id(&*lv.host));
    let mut c = Catalyst {
        water: matches!(
            fluid.as_deref(),
            Some("minecraft:water" | "minecraft:flowing_water")
        ),
        lava: matches!(
            fluid.as_deref(),
            Some("minecraft:lava" | "minecraft:flowing_lava")
        ),
        splashing: lv.block_tagged(p, "create:fan_processing_catalysts/splashing"),
        haunting: lv.block_tagged(p, "create:fan_processing_catalysts/haunting"),
        smoking: lv.block_tagged(p, "create:fan_processing_catalysts/smoking"),
        blasting: lv.block_tagged(p, "create:fan_processing_catalysts/blasting"),
        ..Default::default()
    };
    if !(c.haunting || c.smoking || c.blasting) {
        return fan::kind_at(&c);
    }
    if lv.block_tagged(p, "minecraft:campfires") {
        c.campfire_lit = lv.property(p, "lit").map(|v| v == "true");
    }
    c.soul_flame = lv.property(p, "flame_type").map(|v| v == "soul");
    c.heat = lv.property(p, "blaze").and_then(|v| {
        Some(match v.as_str() {
            "none" => Heat::None,
            "smouldering" => Heat::Smouldering,
            "fading" => Heat::Fading,
            "kindled" => Heat::Kindled,
            "seething" => Heat::Seething,
            _ => return None,
        })
    });
    fan::kind_at(&c)
}

/// `DepotBehaviour.applyToAllItems` with `FanProcessing.applyProcessing`: the held stack's
/// progress, and when done, its results: the first held (centred), the rest in the outputs,
/// what finds no room dropped; none, and the stack is gone.
fn fan_on_depot(lv: &mut ServerLevel<'_>, at: [i32; 3], kind: Kind) -> bool {
    let dim = lv.dim;
    let Some(Some(mut held)) = with_world(|w| w.dims[dim].depots.get(&at).copied()).flatten()
    else {
        return false;
    };
    let Some(slots) = lv.slots(at, depot::SLOTS) else {
        return false;
    };
    let Some(stack) = slots[0].clone() else {
        return false;
    };
    // `getWorldPositionOf`: the depot's centre.
    processing_particles(lv, kind, at.map(|v| f64::from(v) + 0.5));
    let host = &*lv.host;
    let item = stacks::id(host, &stack);
    let count = stack.count().min(255) as u8;
    let fire_resistant = fire_resistant(lv, &stack);
    let can =
        recipes::with(|r| fan::can_process(kind, &item, fire_resistant, r.fan())) == Some(true);
    let done = fan::advance(&mut held.fan, kind, u32::from(count), can);
    if !done {
        with_world(|w| w.dims[dim].depots.insert(at, Some(held)));
        return false;
    }
    let mut rng = processing::Rng::new(lv.seed_at(at));
    let max = |i: &str| stacks::limit(host, i);
    let results =
        recipes::with(|r| fan::process(kind, &item, count, r.fan(), max, &mut rng)).flatten();
    let Some(results) = results else {
        with_world(|w| w.dims[dim].depots.insert(at, Some(held)));
        return false;
    };
    let mut first = None;
    let mut outputs: Vec<processing::Slot> = slots[depot::OUTPUTS]
        .iter()
        .map(|s| stacks::slot_of(host, s))
        .collect();
    let mut dropped = Vec::new();
    for (i, n) in &results {
        if first.is_none() {
            first = stacks::stack(host, i, u32::from(*n));
            continue;
        }
        let left = processing::insert_stacked(&mut outputs, i, *n, max(i));
        if left > 0 {
            dropped.push((i.clone(), left));
        }
    }
    let mut changes = vec![(0, first)];
    for (k, s) in outputs.iter().enumerate() {
        let i = depot::OUTPUTS.start + k;
        if stacks::slot_of(host, &slots[i]) != *s {
            changes.push((i, stacks::from_slot(host, s)));
        }
    }
    lv.set_slots(at, &changes);
    for (i, n) in dropped {
        let p = [
            f64::from(at[0]) + 0.5,
            f64::from(at[1] + 1),
            f64::from(at[2]) + 0.5,
        ];
        lv.spawn_items(p, &i, u32::from(n));
    }
    // getSimilar keeps the motion and the progress (done: -1, never again by this type).
    let next = (!results.is_empty()).then_some(depot::Held {
        position: 0.5,
        ..held
    });
    with_world(|w| {
        w.dims[dim].depots.insert(at, next);
        w.depot_changed(dim, at);
    });
    true
}

/// `BeltBlockEntity.applyToAllItems` with `FanProcessing.applyProcessing`: the items over the
/// segment at `at` (within .51 of its middle).
fn fan_on_belt(lv: &mut ServerLevel<'_>, at: [i32; 3], kind: Kind) -> bool {
    let dim = lv.dim;
    let Some((c, index)) = with_world(|w| {
        let d = &w.dims[dim];
        let c = *d.belt_of.get(&at)?;
        let index = d.belts.get(&c)?.chain.iter().position(|p| *p == at)? as f32;
        let any = d.belts[&c]
            .items
            .items
            .iter()
            .any(|i| (index + 0.5 - i.position).abs() < 0.51);
        any.then_some((c, index))
    })
    .flatten() else {
        return false;
    };
    // `getWorldPositionOf` for each item over the segment: here its middle.
    let over = with_world(|w| {
        w.dims[dim].belts[&c]
            .items
            .items
            .iter()
            .filter(|i| (index + 0.5 - i.position).abs() < 0.51)
            .count()
    })
    .unwrap_or(0);
    for _ in 0..over {
        processing_particles(lv, kind, at.map(|v| f64::from(v) + 0.5));
    }
    let Some(mut inv) = with_world(|w| {
        w.dims[dim]
            .belts
            .get_mut(&c)
            .map(|b| std::mem::take(&mut b.items))
    })
    .flatten() else {
        return false;
    };
    let mut rng = processing::Rng::new(lv.seed_at(at));
    let mut made = 0;
    let lvr = &*lv;
    let host = &*lvr.host;
    let max = |i: &str| stacks::limit(host, i);
    let before = inv.clone();
    let dirty = recipes::with(|r| {
        inv.apply_within(index + 0.5, 0.51, |it| {
            // A stack with components (named by them) is processed as its item.
            let id = create_engine::arm::item_id(&it.item);
            let fire_resistant = stacks::key(host, id).is_some_and(|k| {
                host.item_default_component(k, "minecraft:damage_resistant")
                    .is_some()
            });
            let can = fan::can_process(kind, id, fire_resistant, r.fan());
            if !fan::advance(&mut it.fan, kind, u32::from(it.count), can) {
                return belt::Process::Pass;
            }
            let Some(results) = fan::process(kind, id, it.count, r.fan(), max, &mut rng) else {
                return belt::Process::Pass;
            };
            let outputs = results
                .into_iter()
                .map(|(item, count)| {
                    // getSimilar: a new stack's angle (`R.nextInt(360)`), the old one's place
                    // and progress.
                    made += 1;
                    let angle = random_angle(lvr, [at[0] + made, at[1], at[2]]);
                    let mut out = belt::Item::new(&item, count, angle, upright(host, &item));
                    out.position = it.position;
                    out.inserted_at = it.inserted_at;
                    out.inserted_from = it.inserted_from;
                    out.fan = it.fan;
                    out
                })
                .collect();
            belt::Process::Convert {
                held: None,
                outputs,
            }
        })
    })
    .unwrap_or(false);
    let saved_changed = !inv.saved_eq(&before);
    with_world(|w| {
        if let Some(b) = w.dims[dim].belts.get_mut(&c) {
            b.items = inv;
        }
        if saved_changed {
            w.belt_inventory_changed(dim, c);
        }
        if dirty {
            w.dims[dim].belt_changes.push(c);
        }
    });
    dirty
}
