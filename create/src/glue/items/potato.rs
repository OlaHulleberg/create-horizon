//! The potato cannon (`PotatoCannonItem`, `PotatoProjectileEntity`; the rules are
//! `create_engine::potato`'s): a use fires the first ammunition found (the off hand, the main
//! hand, then the inventory, as `Player.getProjectile` looks) from the barrel beside the eyes,
//! split shots fanned round the aim, with the type's reload as the item's cooldown, the
//! cannon's durability or a backtank's air paid, and `FWOOMP`. Each projectile is a vanilla
//! snowball carrying the ammunition's item, so every client draws it as that item
//! (`ThrownItemRenderer`, as Create's `PotatoProjectileRenderer` draws its stack); the glue
//! keeps its flight by entity id: Create's gravity and drag each tick (set over the snowball's
//! own), sticking to what a sticky type hits, and its hits (`projectile_impact`, taken from the
//! snowball): damage, knockback, the type's entity and block actions, the item recovered or
//! its leftover dropped, `POTATO_HIT` and the item's crumbs.
//!
//! Left out, for want of API:
//! - Create's own entity type (`create:potato_projectile`) and damage type
//!   (`create:potato_cannon`): the shot is a snowball and hurts as `minecraft:thrown` from it and
//!   the shooter. A save keeps the snowballs, not the glue's flights: one flying when the world
//!   is saved comes back as a plain snowball showing its item.
//! - `CureZombieVillager`'s cure (the zombie villager's `mobInteract` with a golden apple from a
//!   fake player): a golden apple gives its food effects to anything it hits.
//! - A powered wither's immunity, knockback resistance, the main arm (the barrel is on the right
//!   of a right-handed player), the swap between two cannons (`SHAPER_SWAP`), the recoil and
//!   barrel particles (`PotatoCannonPacket`), the air-flow trail and the advancements.
//!
//! The snowball is summoned with its item (`/summon`'s compound): a projectile spawned from a
//! rotation carries its own type's item. During its hit on an entity the snowball can't be
//! reached (`host.entity` answers None), so that hit is applied by its id and the snowball's own
//! hit ends it: a sticky shot doesn't stick to an entity, and an item left behind is dropped
//! where the hit was.

use super::super::server::ServerLevel;
use super::super::subsystem::Subsystem;
use super::{creative, held, set_held};
use create_engine::potato::{self, Type};
use mod_api::{
    ActiveEffect, ClientHost, Compound, EntityRef, EquipmentSlot, Flow, Hand, Host, HostSide,
    Identifier, InteractionResult, ItemPart, ItemStack, ParticleKey, ParticleOptions, PlayerRef,
    ProjectileImpactEvent, RandomSource, Registrar, RegistrationContext, ServerHost, SoundSource,
    Tag, Text, TooltipEvent, UseItemEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

const CANNON: &str = "create:potato_cannon";
/// The vanilla thrown item each shot flies as.
const SHOT: &str = "minecraft:snowball";
/// The tag a shot carries, for the shots of a save the glue no longer tracks.
const TAG: &str = "create_potato_projectile";
/// `CEquipment.maxPotatoCannonShots`: shots a full backtank pays for.
const MAX_USES: i32 = potato::MAX_SHOTS;
/// The snowball's own gravity and drag (`ThrowableProjectile`: gravity, then inertia, then the
/// move), undone so that it moves by Create's motion.
const SNOWBALL_GRAVITY: f64 = 0.03;
const SNOWBALL_DRAG: f64 = 0.99;

static ITEM_PARTICLE: OnceLock<ParticleKey> = OnceLock::new();

/// A shot in flight, by the snowball's entity id.
struct Shot {
    dimension: String,
    kind: &'static Type,
    /// The ammunition, one of it.
    stack: ItemStack,
    recovery: f32,
    owner: Option<EntityRef>,
    /// The motion Create's projectile has (the snowball's is set so that it moves by it).
    motion: [f64; 3],
    /// Stuck to an entity: it, the offset from it and the fall speed (`setStuckEntity`).
    stuck: Option<(EntityRef, [f64; 3], f64)>,
    /// `tickCount`.
    age: i32,
}

thread_local! {
    static SHOTS: RefCell<HashMap<u32, Shot>> = RefCell::new(HashMap::new());
}

pub struct PotatoCannon;

impl Subsystem for PotatoCannon {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let _ = ITEM_PARTICLE.set(r.particle_key("minecraft:item"));
        let key = r.item_key(CANNON);
        r.item_part(key, CannonPart);
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        fly(lv);
    }

    fn hears_projectile_impacts(&self) -> bool {
        true
    }

    fn projectile_impact(
        &self,
        host: &mut dyn ServerHost,
        event: &mut ProjectileImpactEvent<'_>,
    ) -> Flow {
        let Some(shot) = SHOTS.with_borrow_mut(|s| s.remove(&event.projectile)) else {
            return Flow::Continue;
        };
        // During an entity hit the snowball can't be reached (`host.entity` answers None for
        // it): the hit is applied here and the snowball's own hit ends it.
        event.cancelled = host.entity(event.projectile).is_some();
        let keep = impact(host, event, shot);
        if let Some(shot) = keep {
            SHOTS.with_borrow_mut(|s| s.insert(event.projectile, shot));
        }
        Flow::Cancel
    }
}

struct CannonPart;

#[mod_api::horizon_part]
impl ItemPart for CannonPart {
    /// `PotatoCannonItem.use`: passes without ammunition.
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Server(host) => fire(host, event).then_some(InteractionResult::Success),
            HostSide::Client(host) => {
                let player = host.player(event.player)?;
                ammo(&*host, player).map(|_| InteractionResult::Success)
            }
        }
    }

    /// `PotatoCannonItem.appendHoverText`: the ammunition found and what it does.
    fn tooltip(&self, host: &dyn ClientHost, event: &mut TooltipEvent<'_>) {
        let Some(player) = host.local_player() else {
            return;
        };
        let Some((_, stack, t)) = ammo(host, player) else {
            return;
        };
        const DARK_GREEN: u32 = 0xFF00_AA00;
        const GRAY: u32 = 0xFFAA_AAAA;
        let line = |key: &str, value: String| {
            let text = host
                .translate(&format!("create.potato_cannon.ammo.{key}"))
                .unwrap_or_default()
                .replace("%1$s", &value);
            (Text::literal(format!(" {text}")), DARK_GREEN)
        };
        event.lines.push((Text::literal(""), u32::MAX));
        event
            .lines
            .push((Text::literal(format!("{}:", host.hover_name(&stack))), GRAY));
        event
            .lines
            .push(line("attack_damage", t.damage.to_string()));
        event
            .lines
            .push(line("reload_ticks", t.reload_ticks.to_string()));
        event.lines.push(line("knockback", t.knockback.to_string()));
    }
}

/// Where the ammunition is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Hand(Hand),
    Slot(u32),
}

/// `Player.getProjectile` for the cannon: the off hand, the main hand, then the inventory's
/// slots in order; the first stack Create has a projectile type for.
fn ammo(host: &dyn Host, player: PlayerRef) -> Option<(Source, ItemStack, &'static Type)> {
    let kind = |s: &ItemStack| potato::type_for(&s.item().id(host));
    for hand in [Hand::Off, Hand::Main] {
        if let Some(stack) = held(host, player, hand)
            && let Some(t) = kind(&stack)
        {
            return Some((Source::Hand(hand), stack, t));
        }
    }
    let mut slots = player.inventory_slots(host)?;
    slots.sort_by_key(|(slot, _)| *slot);
    slots
        .into_iter()
        .filter(|(slot, _)| *slot < 36)
        .find_map(|(slot, stack)| kind(&stack).map(|t| (Source::Slot(slot), stack, t)))
}

/// `Entity.isAlive`: a living entity's, or any other's presence in its level.
fn alive(host: &dyn Host, e: EntityRef) -> bool {
    match host.living(e) {
        Some(l) => l.alive(host) == Some(true),
        None => e.added_to_level(host),
    }
}

/// One of `stack`.
fn one(stack: &ItemStack) -> ItemStack {
    let mut s = stack.clone();
    let _ = s.set_count(1);
    s
}

/// `Create.RANDOM.nextFloat()`, from the level's random.
fn random(lv: &mut ServerLevel<'_>) -> f32 {
    lv.level
        .with_random(&mut *lv.host, |r| r.next_float())
        .unwrap_or(0.5)
}

/// The cannon's `potato_recovery` enchantment level.
fn recovery_level(stack: &ItemStack) -> i32 {
    let levels = stack
        .components()
        .and_then(|c| c.as_nbt().get(&"minecraft:enchantments".into()).cloned());
    match levels {
        Some(Tag::Compound(c)) => match c.get(&"create:potato_recovery".into()) {
            Some(Tag::Int(v)) => *v,
            _ => 0,
        },
        _ => 0,
    }
}

/// The velocity to give the snowball so that its own gravity and drag leave it moving by
/// `motion` this tick.
fn snowball_velocity(motion: [f64; 3]) -> [f64; 3] {
    [
        motion[0] / SNOWBALL_DRAG,
        motion[1] / SNOWBALL_DRAG + SNOWBALL_GRAVITY,
        motion[2] / SNOWBALL_DRAG,
    ]
}

/// `PotatoCannonItem.use` on the server.
fn fire(host: &mut dyn ServerHost, event: &UseItemEvent<'_>) -> bool {
    let Some(player) = host.player(event.player) else {
        return false;
    };
    let Some((source, ammo_stack, t)) = ammo(&*host, player) else {
        return false;
    };
    let entity = player.entity();
    let (Some(living), Some(eye), Some([yaw, pitch]), Some(look)) = (
        host.living(entity),
        entity.eye_position(&*host),
        entity.rotation(&*host),
        entity.view_vector(&*host),
    ) else {
        return false;
    };
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return false;
    };
    let main = event.hand == Hand::Main;
    let barrel = potato::barrel(eye, yaw, pitch, main, [0.75, -0.15, 1.5]);
    let side = potato::barrel(eye, yaw, pitch, main, [-0.05, 0.0, 0.0]);
    let correction = [side[0] - eye[0], side[1] - eye[1], side[2] - eye[2]];
    let motion = potato::motion(look, correction, t);
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return false;
    };
    let pitch_shift = t.sound_pitch + (random(&mut lv) - 0.5) / 4.0;
    let base = 360.0 * random(&mut lv);
    let imperfections: Vec<f32> = (0..t.split)
        .map(|_| 40.0 * (random(&mut lv) - 0.5))
        .collect();
    let motions = potato::spray(motion, t, base, &imperfections);
    let cannon = held(&*lv.host, player, event.hand);
    let recovery = cannon
        .as_ref()
        .map_or(0.0, |c| potato::recovery_chance(recovery_level(c)));
    let shot_stack = one(&ammo_stack);
    let Some(item) = shot_stack.to_nbt(&*lv.host) else {
        return false;
    };
    // The owner as `/summon`'s int-array UUID.
    let owner = player.uuid();
    let uuid: Vec<i32> = (0..4)
        .map(|i| (owner >> (96 - 32 * i)) as u32 as i32)
        .collect();
    for (i, m) in motions.into_iter().enumerate() {
        // Create's first tick moves it by `fly(motion)`, from the barrel. A thrown item of
        // vanilla's showing the ammo (`ThrowableItemProjectile`'s `Item`), summoned whole: the
        // projectile spawn takes no item to show.
        let first = potato::fly(m, t);
        let v = snowball_velocity(first);
        let nbt = Compound::from([
            ("Item".into(), Tag::Compound(item.clone())),
            ("Owner".into(), Tag::IntArray(uuid.clone())),
            ("Tags".into(), Tag::List(vec![Tag::String(TAG.into())])),
            (
                "Motion".into(),
                Tag::List(v.iter().map(|c| Tag::Double(c.to_bits())).collect()),
            ),
        ]);
        let Some(e) = lv
            .host
            .summon(&dimension, SHOT, barrel, nbt)
            .and_then(|id| lv.host.entity(id))
        else {
            crate::create_log!("[create] a potato cannon's shot was refused");
            continue;
        };
        SHOTS.with_borrow_mut(|s| {
            s.insert(
                e.id(),
                Shot {
                    dimension: dimension.clone(),
                    kind: t,
                    stack: shot_stack.clone(),
                    recovery: if i == 0 { recovery } else { 0.0 },
                    owner: Some(entity),
                    motion: first,
                    stuck: None,
                    age: 0,
                },
            )
        });
    }
    if !creative(&*lv.host, player) {
        match source {
            Source::Hand(hand) => set_held(lv.host, player, hand, ammo_stack.shrink(1)),
            Source::Slot(slot) => {
                let _ = player.set_inventory_slot(lv.host, slot, ammo_stack.shrink(1));
            }
        }
    }
    if !super::super::equipment::absorb_damage(lv.host, player, MAX_USES) {
        let slot = if main {
            EquipmentSlot::MainHand
        } else {
            EquipmentSlot::OffHand
        };
        let _ = living.hurt_and_break(lv.host, slot, 1);
    }
    let other = if main { Hand::Off } else { Hand::Main };
    let gun_in_other_hand =
        held(&*lv.host, player, other).is_some_and(|s| s.item().id(&*lv.host) == CANNON);
    let cooldown = potato::cooldown(t, gun_in_other_hand);
    if cooldown > 0 {
        player.set_item_cooldown(lv.host, CANNON, cooldown);
    }
    // `PotatoCannonPacket`: `FWOOMP` at the barrel for everyone.
    lv.level.play_sound(
        &mut *lv.host,
        "create:fwoomp",
        barrel,
        SoundSource::Players,
        1.0,
        pitch_shift,
    );
    true
}

/// `PotatoProjectileEntity.tick` for this level's shots.
fn fly(lv: &mut ServerLevel<'_>) {
    let ids: Vec<u32> = SHOTS.with_borrow(|s| {
        s.iter()
            .filter(|(_, shot)| shot.dimension == lv.dimension)
            .map(|(id, _)| *id)
            .collect()
    });
    for id in ids {
        let Some(mut shot) = SHOTS.with_borrow_mut(|s| s.remove(&id)) else {
            continue;
        };
        let Some(e) = lv.host.entity(id).filter(|e| e.added_to_level(&*lv.host)) else {
            continue;
        };
        shot.age += 1;
        let stuck = shot.stuck.filter(|(target, ..)| alive(&*lv.host, *target));
        if let Some((target, mut offset, mut fall)) = stuck {
            let (Some(at), Some(own)) = (target.position(&*lv.host), e.position(&*lv.host)) else {
                continue;
            };
            if own[1] < at[1] - 0.1 {
                pop(lv, &shot, own);
                e.discard(lv.host);
                continue;
            }
            fall += 0.007 * f64::from(shot.kind.gravity);
            offset[1] -= fall;
            e.teleport_to(
                lv.host,
                [at[0] + offset[0], at[1] + offset[1], at[2] + offset[2]],
                None,
            );
            e.set_velocity(lv.host, [0.0; 3]);
            shot.stuck = Some((target, offset, fall));
        } else {
            shot.stuck = None;
            shot.motion = potato::fly(shot.motion, shot.kind);
            e.set_velocity(lv.host, snowball_velocity(shot.motion));
        }
        SHOTS.with_borrow_mut(|s| s.insert(id, shot));
    }
}

/// `pop`: the item's crumbs and `POTATO_HIT` where it hit.
fn pop(lv: &mut ServerLevel<'_>, shot: &Shot, at: [f64; 3]) {
    if let Some(key) = ITEM_PARTICLE.get() {
        let options = ParticleOptions::item_stack(*key, shot.stack.clone());
        lv.level
            .send_particles(&mut *lv.host, &options, None, at, [0.0; 3], 0.25, 7);
    }
    lv.level.play_sound(
        &mut *lv.host,
        "create:potato_hit",
        at,
        SoundSource::Neutral,
        1.0,
        1.0,
    );
}

/// `recoverItem` or `spawnAtLocation(type.dropStack())` where the shot is.
fn leave_item(lv: &mut ServerLevel<'_>, projectile: EntityRef, shot: &Shot) {
    let stack = if f64::from(random(lv)) <= f64::from(shot.recovery) {
        Some(shot.stack.clone())
    } else {
        shot.kind.drop.and_then(|id| {
            let key = lv.host.item_key(&Identifier::parse(id).ok()?)?;
            ItemStack::new(key, 1)
        })
    };
    if let Some(stack) = stack {
        let _ = projectile.spawn_at_location(lv.host, stack, 0.0);
    }
}

/// `leave_item` where an entity was hit, the snowball out of reach: the item summoned there.
fn leave_item_at(lv: &mut ServerLevel<'_>, at: [f64; 3], shot: &Shot) {
    let stack = if f64::from(random(lv)) <= f64::from(shot.recovery) {
        Some(shot.stack.clone())
    } else {
        shot.kind.drop.and_then(|id| {
            let key = lv.host.item_key(&Identifier::parse(id).ok()?)?;
            ItemStack::new(key, 1)
        })
    };
    if let Some(item) = stack.and_then(|s| s.to_nbt(&*lv.host)) {
        let nbt = Compound::from([("Item".into(), Tag::Compound(item))]);
        let dimension = lv.dimension.to_owned();
        lv.host.summon(&dimension, "minecraft:item", at, nbt);
    }
}

/// `onHitEntity` / `onHitBlock`; the shot back when it flies on or sticks.
fn impact(
    host: &mut dyn ServerHost,
    event: &ProjectileImpactEvent<'_>,
    mut shot: Shot,
) -> Option<Shot> {
    let projectile = host.entity(event.projectile);
    let mut lv = ServerLevel::new(host, event.dimension)?;
    if shot.stuck.is_some() {
        return Some(shot);
    }
    let Some(target_id) = event.entity else {
        let projectile = projectile?;
        // `onHitBlock`.
        pop(&mut lv, &shot, event.at);
        if !block_hit(&mut lv, &shot, event) {
            leave_item(&mut lv, projectile, &shot);
        }
        projectile.discard(lv.host);
        return None;
    };
    let target = lv.host.entity(target_id)?;
    if !alive(&*lv.host, target) {
        return Some(shot);
    }
    // Two shots meeting just out of their barrels pass through each other.
    let young = SHOTS.with_borrow(|s| s.get(&target_id).map(|o| o.age < 10));
    if young == Some(true) && shot.age < 10 {
        return Some(shot);
    }
    pop(&mut lv, &shot, event.at);
    // `preEntityHit`: set on fire.
    if let Some(ticks) = action_arg(shot.kind, "set_on_fire").and_then(|a| a.parse().ok()) {
        target.set_remaining_fire_ticks(lv.host, ticks);
    }
    let enderman = event.entity_kind == "minecraft:enderman";
    let (damage, knockback, invulnerable) = potato::hit(shot.kind);
    let hurt = target.hurt(lv.host, "minecraft:thrown", damage, projectile, shot.owner);
    if hurt != Some(true) {
        discard(&mut lv, projectile);
        return None;
    }
    if enderman {
        return Some(shot);
    }
    if !entity_hit(&mut lv, &shot, target) {
        leave_item_at(&mut lv, event.at, &shot);
    }
    // A thing not living (a boat, a minecart) only takes the hit.
    if lv.host.living(target).is_none() {
        discard(&mut lv, projectile);
        return None;
    }
    if let Some(ticks) = invulnerable {
        target.set_invulnerable_time(lv.host, ticks);
    }
    let velocity = target.velocity(&*lv.host).unwrap_or([0.0; 3]);
    let ground = target.on_ground(&*lv.host) == Some(true);
    if let Some(v) = potato::knockback(velocity, shot.motion, knockback, ground) {
        target.set_velocity(lv.host, v);
    }
    if shot.kind.sticky
        && alive(&*lv.host, target)
        && let Some(projectile) = projectile
    {
        let (Some(at), Some(own)) = (target.position(&*lv.host), projectile.position(&*lv.host))
        else {
            projectile.discard(lv.host);
            return None;
        };
        projectile.set_velocity(lv.host, [0.0; 3]);
        shot.stuck = Some((
            target,
            [own[0] - at[0], own[1] - at[1], own[2] - at[2]],
            0.0,
        ));
        return Some(shot);
    }
    discard(&mut lv, projectile);
    None
}

/// The snowball gone, when it can be reached (else its own hit ends it).
fn discard(lv: &mut ServerLevel<'_>, projectile: Option<EntityRef>) {
    if let Some(p) = projectile {
        p.discard(lv.host);
    }
}

/// The argument of the type's action named `name` (`"set_on_fire 60"` gives `"60"`).
fn action_arg(t: &Type, name: &str) -> Option<&'static str> {
    let action = t.action?;
    let rest = action.strip_prefix(name)?;
    Some(rest.trim_start())
}

/// `MobEffectInstance(effect, ticks, amplifier)` on `target`.
fn give(lv: &mut ServerLevel<'_>, target: EntityRef, effect: &str, ticks: i32, amplifier: i32) {
    let (Some(living), Some(key)) = (lv.host.living(target), lv.host.effect_key(effect)) else {
        return;
    };
    living.add_effect(
        lv.host,
        &ActiveEffect::new(key, ticks, amplifier, false, true, true),
    );
}

/// `onEntityHit`: true when the type took care of the item (nothing recovered or dropped).
fn entity_hit(lv: &mut ServerLevel<'_>, shot: &Shot, target: EntityRef) -> bool {
    let t = shot.kind;
    let Some(action) = t.action else {
        return false;
    };
    let mut words = action.split_whitespace();
    match words.next() {
        // `PotionEffect`: recoverable for all but the glow berry.
        Some("potion_effect") => {
            let (Some(effect), Some(level), Some(ticks)) = (
                words.next(),
                words.next().and_then(|w| w.parse::<i32>().ok()),
                words.next().and_then(|w| w.parse::<i32>().ok()),
            ) else {
                return false;
            };
            give(lv, target, effect, ticks, level - 1);
            t.id == "glow_berry"
        }
        // `FoodEffects` of the food, not recoverable.
        Some("food_effects") => {
            let effects: &[(&str, i32, i32)] = match t.id {
                "enchanted_golden_apple" => &[
                    ("minecraft:regeneration", 400, 1),
                    ("minecraft:resistance", 6000, 0),
                    ("minecraft:fire_resistance", 6000, 0),
                    ("minecraft:absorption", 2400, 3),
                ],
                "pufferfish" => &[
                    ("minecraft:poison", 1200, 1),
                    ("minecraft:hunger", 300, 2),
                    ("minecraft:nausea", 300, 0),
                ],
                _ => &[],
            };
            for (effect, ticks, amplifier) in effects {
                give(lv, target, effect, *ticks, *amplifier);
            }
            true
        }
        // `CureZombieVillager`: a golden apple's own effects (`Foods.GOLDEN_APPLE`).
        Some("cure_zombie_villager") => {
            give(lv, target, "minecraft:regeneration", 100, 1);
            give(lv, target, "minecraft:absorption", 2400, 0);
            true
        }
        Some("chorus_teleport") => {
            let diameter = words
                .next()
                .and_then(|w| w.parse::<f64>().ok())
                .unwrap_or(20.0);
            chorus(lv, target, diameter)
        }
        Some("suspicious_stew") => {
            for (effect, ticks) in stew_effects(&shot.stack) {
                give(lv, target, &effect, ticks, 0);
            }
            false
        }
        _ => false,
    }
}

/// `SuspiciousStewEffects` of the stack: each effect and its duration.
fn stew_effects(stack: &ItemStack) -> Vec<(String, i32)> {
    let list = stack.components().and_then(|c| {
        c.as_nbt()
            .get(&"minecraft:suspicious_stew_effects".into())
            .cloned()
    });
    let Some(Tag::List(list)) = list else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|e| {
            let Tag::Compound(c) = e else {
                return None;
            };
            let id = match c.get(&"id".into())? {
                Tag::String(s) => s.to_string_lossy(),
                _ => return None,
            };
            let ticks = match c.get(&"duration".into()) {
                Some(Tag::Int(d)) => *d,
                // `SuspiciousEffectHolder.EFFECT_DURATION`'s default.
                _ => 160,
            };
            Some((id, ticks))
        })
        .collect()
}

/// `ChorusTeleport`: up to sixteen tries of `randomTeleport` within `diameter`.
fn chorus(lv: &mut ServerLevel<'_>, target: EntityRef, diameter: f64) -> bool {
    let (Some(living), Some(at)) = (lv.host.living(target), target.position(&*lv.host)) else {
        return false;
    };
    let fox = target.kind(&*lv.host).as_deref() == Some("minecraft:fox");
    let sound = if fox {
        "minecraft:entity.fox.teleport"
    } else {
        "minecraft:item.chorus_fruit.teleport"
    };
    let span = diameter as i32;
    for _ in 0..16 {
        let (rx, rz) = (f64::from(random(lv)), f64::from(random(lv)));
        let ry = f64::from((random(lv) * span.max(1) as f32) as i32);
        let to = [
            at[0] + (rx - 0.5) * diameter,
            (at[1] + ry - f64::from(span / 2)).max(0.0),
            at[2] + (rz - 0.5) * diameter,
        ];
        if living.random_teleport(lv.host, to, None) {
            lv.level
                .play_sound(&mut *lv.host, sound, at, SoundSource::Players, 1.0, 1.0);
            target.set_velocity(lv.host, [0.0; 3]);
            return true;
        }
    }
    false
}

/// `onBlockHit`: true when the type took care of the item.
fn block_hit(lv: &mut ServerLevel<'_>, shot: &Shot, event: &ProjectileImpactEvent<'_>) -> bool {
    let Some(action) = shot.kind.action else {
        return false;
    };
    let mut words = action.split_whitespace();
    let (Some(kind), Some(block)) = (words.next(), words.next()) else {
        return false;
    };
    let face = event.face;
    let place = [
        event.block[0] + face[0],
        event.block[1] + face[1],
        event.block[2] + face[2],
    ];
    let replaceable = lv.is_air(place)
        || lv.block_state(place).is_some_and(|s| {
            lv.host.block_in_tag(s.block(), "minecraft:replaceable") == Some(true)
        });
    match kind {
        // `PlantCrop`: on top of farmland.
        "plant_crop" => {
            if face != [0, 1, 0] || !replaceable {
                return false;
            }
            let farmland = lv
                .block_state(event.block)
                .is_some_and(|s| s.block().id(&*lv.host) == "minecraft:farmland");
            if farmland && let Some(state) = lv.host.block_state_id(block) {
                lv.set_state(place, state, super::super::server::update_all());
            }
            true
        }
        // `PlaceBlockOnGround`: set on top, else falling beside where it hit.
        "place_block_on_ground" => {
            if !replaceable {
                return false;
            }
            if face == [0, 1, 0] {
                if let Some(state) = lv.host.block_state_id(block) {
                    lv.set_state(place, state, super::super::server::update_all());
                }
            } else {
                let empty = |lv: &ServerLevel<'_>, p: [i32; 3]| lv.is_air(p);
                let mut y = event.at[1] - 0.5;
                if !empty(lv, [place[0], place[1] + 1, place[2]]) {
                    y = y.min(f64::from(place[1]));
                }
                if !empty(lv, [place[0], place[1] - 1, place[2]]) {
                    y = y.max(f64::from(place[1]));
                }
                let nbt = Compound::from([
                    (
                        "BlockState".into(),
                        Tag::Compound(Compound::from([("Name".into(), Tag::String(block.into()))])),
                    ),
                    ("Time".into(), Tag::Int(1)),
                ]);
                let at = [f64::from(place[0]) + 0.5, y, f64::from(place[2]) + 0.5];
                let dimension = lv.dimension.to_owned();
                lv.host
                    .summon(&dimension, "minecraft:falling_block", at, nbt);
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snowball_moves_by_creates_motion() {
        // Gravity, then drag, then the move: what is left is the motion given.
        let m = [0.4, -0.2, 1.1];
        let v = snowball_velocity(m);
        let moved = [
            v[0] * SNOWBALL_DRAG,
            (v[1] - SNOWBALL_GRAVITY) * SNOWBALL_DRAG,
            v[2] * SNOWBALL_DRAG,
        ];
        for i in 0..3 {
            assert!((moved[i] - m[i]).abs() < 1e-12);
        }
    }

    #[test]
    fn actions_read_their_arguments() {
        let baked = potato::type_for("minecraft:baked_potato").unwrap();
        assert_eq!(action_arg(baked, "set_on_fire"), Some("60"));
        assert_eq!(action_arg(baked, "plant_crop"), None);
        assert_eq!(action_arg(potato::fallback(), "set_on_fire"), None);
    }
}
