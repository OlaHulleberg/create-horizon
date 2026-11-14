//! Create's potato cannon (`PotatoCannonItem`, `PotatoProjectileEntity`; the rules are
//! `create_engine::potato`): used with ammunition in the off hand or the inventory, it fires
//! `create:potato_projectile` (a thrown item projectile carrying the ammunition, declared as a
//! copy of the snowball) from its barrel at the ammunition's speed, one or a spray of them, with
//! its `fwoomp`; the ammunition is used outside creative, the cannon wears unless a backtank's
//! air pays, and the cannon reloads for the ammunition's ticks. A hit deals the ammunition's
//! damage (as `thrown` damage: a native mod's damage types are not registered), knocks back,
//! shortens the target's invulnerability for fast ammunition, and does what the ammunition
//! does: setting on fire, a potion effect, planting its crop or putting its block on the
//! ground; what it doesn't use may come back with Potato Recovery.
//!
//! Left out: the projectile flies as a snowball does (its gravity and drag, not the
//! ammunition's), it never sticks to what it hits, and the food, chorus fruit, golden apple and
//! suspicious stew effects, the barrel's recoil and the cannon's own item model are not ported.

use super::super::states;
use super::{Equipment, absorb};
use create_engine::potato::{self, Type};
use mod_api::{
    ActiveEffect, ClientHost, EntityRef, Flow, Hand, HostSide, Identifier, InteractionResult,
    ItemPart, ItemStack, LivingRef, PlayerRef, ProjectileImpactEvent, ServerHost, SoundSource, Tag,
    UseItemEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// `AllEntityTypes.POTATO_PROJECTILE`.
pub(super) const PROJECTILE: &str = "create:potato_projectile";

/// A shot fired, to clients: its entity id (four little-endian bytes) and its item's id.
pub(super) const SHOT: &str = "create:potato";

/// What each shot clients see carries, by entity id (the renderer has no host to ask).
static SHOWN: std::sync::Mutex<Vec<(u32, ItemStack)>> = std::sync::Mutex::new(Vec::new());

/// A client heard of a shot: it draws it as its item.
pub(super) fn shot_seen(host: &dyn ClientHost, data: &[u8]) {
    let Some((id, item)) = data.split_first_chunk::<4>() else {
        return;
    };
    let id = u32::from_le_bytes(*id);
    let Some(stack) = std::str::from_utf8(item)
        .ok()
        .and_then(|i| stack_of(host, i))
    else {
        return;
    };
    if let Ok(mut shown) = SHOWN.lock() {
        shown.retain(|(i, _)| *i != id);
        if shown.len() >= 256 {
            shown.remove(0);
        }
        shown.push((id, stack));
    }
}

/// `PotatoProjectileRenderer`: the shot's item, turned to its flight and tumbling.
pub(super) struct Shown;

impl mod_api::EntityRenderer for Shown {
    fn submit(
        &self,
        entity: &mod_api::EntityRenderState,
        draw: &mut dyn mod_api::BlockEntityDrawContext,
    ) -> Result<(), String> {
        let stack = SHOWN.lock().ok().and_then(|s| {
            s.iter()
                .find(|(i, _)| *i == entity.id)
                .map(|(_, st)| st.clone())
        });
        let Some(stack) = stack else {
            return Ok(());
        };
        draw.push();
        draw.translate([0.0, 0.125, 0.0]);
        draw.rotate_about([0.0, 1.0, 0.0], -entity.yaw, [0.0; 3]);
        draw.rotate_about([1.0, 0.0, 0.0], entity.age * 20.0, [0.0; 3]);
        draw.item(
            &stack,
            mod_api::ItemDisplayContext::Ground,
            None,
            entity.id as i32,
        );
        draw.pop();
        Ok(())
    }
}

/// A shot in flight: what it carries, its type, who shot it and the chance its item comes back.
#[derive(Clone)]
struct Shot {
    item: String,
    kind: &'static Type,
    owner: Option<EntityRef>,
    recovery: f32,
}

thread_local! {
    /// The potato projectiles flying, by entity id.
    static FLYING: RefCell<HashMap<u32, Shot>> = RefCell::new(HashMap::new());
}

pub(super) fn reset() {
    FLYING.with_borrow_mut(HashMap::clear);
}

/// Whether projectiles' impacts are to be heard: while potato shots fly.
pub(super) fn hears() -> bool {
    FLYING.with_borrow(|f| !f.is_empty())
}

pub(super) struct Cannon;

#[mod_api::horizon_part]
impl ItemPart for Cannon {
    /// `PotatoCannonItem.use`: fire with ammunition, else nothing (always answered, so the
    /// projectile type's own throwing of the cannon never runs).
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Client(host) => Some(predict(host, event)),
            HostSide::Server(host) => Some(fire(host, event)),
        }
    }

    /// `isBarVisible` / `getBarWidth` (`BacktankUtil`): the worn backtanks' air while there is
    /// any, else the cannon's wear.
    fn item_bar(&self, host: &dyn ClientHost, event: &mut mod_api::ItemBarEvent<'_>) {
        let Some(player) = host.local_player() else {
            return;
        };
        let Some(living) = host.living(player.entity()) else {
            return;
        };
        let (mut air, mut max) = (0, 0);
        for slot in super::ARMOR {
            if let Some(s) = living.equipment(host, slot)
                && matches!(
                    super::gear_of(host, &s),
                    create_engine::equipment::Gear::Backtank(_)
                )
            {
                let cap = super::capacity(&s);
                air += create_engine::equipment::air(super::stack_air(&s), cap);
                max += create_engine::equipment::max_air(cap);
            }
        }
        if air > 0 && max > 0 {
            let width = (13.0 * (air as f32 / max as f32).clamp(0.0, 1.0)).round() as i32;
            *event.bar = Some((width, super::BAR_COLOR));
        }
    }
}

/// `getAmmo` (`Player.getProjectile`): the off hand's stack, then the main hand's, then the
/// inventory's in order, the first an ammunition type lists.
fn ammo(host: &dyn mod_api::Host, player: PlayerRef) -> Option<(Option<Hand>, u32, ItemStack)> {
    for hand in [Hand::Off, Hand::Main] {
        if let Some(s) = super::super::items::held(host, player, hand)
            && potato::type_for(&s.item().id(host)).is_some()
        {
            return Some((Some(hand), 0, s));
        }
    }
    player
        .inventory_slots(host)
        .unwrap_or_default()
        .into_iter()
        .find(|(_, s)| potato::type_for(&s.item().id(host)).is_some())
        .map(|(i, s)| (None, i, s))
}

fn predict(host: &mut dyn ClientHost, event: &UseItemEvent<'_>) -> InteractionResult {
    let Some(player) = host.player(event.player) else {
        return InteractionResult::Pass;
    };
    if ammo(&*host, player).is_some() {
        InteractionResult::Success
    } else {
        InteractionResult::Pass
    }
}

/// `PotatoCannonItem.use` on the server.
fn fire(host: &mut dyn ServerHost, event: &UseItemEvent<'_>) -> InteractionResult {
    let Some(player) = host.player(event.player) else {
        return InteractionResult::Pass;
    };
    let Some((ammo_hand, slot, ammo_stack)) = ammo(&*host, player) else {
        return InteractionResult::Pass;
    };
    let item = ammo_stack.item().id(&*host);
    let Some(kind) = potato::type_for(&item) else {
        return InteractionResult::Pass;
    };
    let entity = player.entity();
    let (Some(living), Some(eye), Some([yaw, pitch])) = (
        host.living(entity),
        entity.eye_position(&*host),
        entity.rotation(&*host),
    ) else {
        return InteractionResult::Pass;
    };
    let right = event.hand == Hand::Main;
    let barrel = potato::barrel(eye, yaw, pitch, right, [0.75, -0.15, 1.5]);
    let correction = {
        let b = potato::barrel(eye, yaw, pitch, right, [-0.05, 0.0, 0.0]);
        [b[0] - eye[0], b[1] - eye[1], b[2] - eye[2]]
    };
    let look = look_vector(yaw, pitch);
    let motion = potato::motion(look, correction, kind);
    let level = player.level(&*host);
    let mut random = || {
        level
            .and_then(|l| {
                l.with_random(&mut *host, |r| {
                    use mod_api::RandomSource;
                    r.next_float()
                })
            })
            .unwrap_or(0.5)
    };
    let pitch_sound = kind.sound_pitch + (random() - 0.5) / 4.0;
    let base = 360.0 * random();
    let imperfections: Vec<f32> = (0..kind.split.max(1))
        .map(|_| 40.0 * (random() - 0.5))
        .collect();
    let motions = potato::spray(motion, kind, base, &imperfections);
    let recovery = potato::recovery_chance(recovery_level(&*host, event.stack));
    let Ok(id) = Identifier::parse(PROJECTILE) else {
        return InteractionResult::Pass;
    };
    let Some(one) = ammo_stack
        .clone()
        .shrink(ammo_stack.count().saturating_sub(1))
    else {
        return InteractionResult::Pass;
    };
    let mut shown = Vec::new();
    for (i, m) in motions.into_iter().enumerate() {
        let speed = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt() as f32;
        let Some(projectile) =
            host.spawn_projectile_from_rotation(living, &id, &one, 0.0, speed, 0.0)
        else {
            continue;
        };
        let e = projectile.entity();
        let _ = e.teleport_to(host, barrel, None);
        let _ = e.set_velocity(host, m);
        let shot = Shot {
            item: item.clone(),
            kind,
            owner: Some(entity),
            recovery: if i == 0 { recovery } else { 0.0 },
        };
        FLYING.with_borrow_mut(|f| f.insert(e.id(), shot));
        shown.push(e.id());
    }
    // Clients draw each shot as its ammunition (`PotatoProjectileRenderer`).
    if let Some(dimension) = player.dimension(&*host).map(|d| d.to_string())
        && let Some(lv) = super::super::server::ServerLevel::new(host, &dimension)
    {
        let players = lv.players();
        for id in shown {
            let mut data = id.to_le_bytes().to_vec();
            data.extend_from_slice(item.as_bytes());
            super::super::net::send_to_players(host, &players, SHOT, &data);
        }
    }
    let creative = super::super::items::creative(&*host, player);
    if !creative {
        match ammo_hand {
            Some(hand) => super::super::items::shrink_held(host, player, hand, 1),
            None => {
                let left = ammo_stack.shrink(1);
                let _ = player.set_inventory_slot(host, slot, left);
            }
        }
    }
    // `canAbsorbDamage(player, maxUses())`, else the cannon wears.
    if !absorb(host, player, potato::MAX_SHOTS) {
        let slot = if right {
            mod_api::EquipmentSlot::MainHand
        } else {
            mod_api::EquipmentSlot::OffHand
        };
        let _ = living.hurt_and_break(host, slot, 1);
    }
    let group = host.item_cooldown_group(event.stack);
    let _ = player.set_item_cooldown(host, &group, kind.reload_ticks);
    // `PotatoCannonPacket`: FWOOMP at the barrel.
    if let Some(level) = level {
        level.play_sound(
            host,
            "create:fwoomp",
            barrel,
            SoundSource::Players,
            1.0,
            pitch_sound,
        );
    }
    InteractionResult::Success
}

/// `Entity.getLookAngle` for `yaw` and `pitch` in degrees.
fn look_vector(yaw: f32, pitch: f32) -> [f64; 3] {
    let (y, p) = (f64::from(yaw).to_radians(), f64::from(pitch).to_radians());
    [-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos()]
}

/// The cannon's Potato Recovery level (`create:potato_recovery`).
fn recovery_level(host: &dyn mod_api::Host, cannon: &ItemStack) -> i32 {
    let _ = host;
    let levels = cannon
        .components()
        .and_then(|c| c.as_nbt().get(&"minecraft:enchantments".into()));
    match levels {
        Some(Tag::Compound(c)) => match c.get(&"create:potato_recovery".into()) {
            Some(Tag::Int(v)) => *v,
            _ => 0,
        },
        _ => 0,
    }
}

impl Equipment {
    /// `PotatoProjectileEntity.onHitEntity` / `onHitBlock` for a potato shot: its own hit, then
    /// the snowball's, which does nothing more and breaks it.
    pub(super) fn potato_impact(
        host: &mut dyn ServerHost,
        event: &mut ProjectileImpactEvent<'_>,
    ) -> Flow {
        if event.kind != PROJECTILE {
            return Flow::Continue;
        }
        let Some(shot) = FLYING.with_borrow_mut(|f| f.remove(&event.projectile)) else {
            return Flow::Continue;
        };
        let projectile = host.entity(event.projectile);
        let Some(level) =
            super::super::server::ServerLevel::new(host, event.dimension).map(|lv| lv.level)
        else {
            return Flow::Continue;
        };
        // `pop`: the item's crumbs and POTATO_HIT.
        level.play_sound(
            host,
            "create:potato_hit",
            event.at,
            SoundSource::Neutral,
            1.0,
            1.0,
        );
        let used = match event.entity.and_then(|id| host.entity(id)) {
            Some(target) => hit_entity(host, &shot, projectile, target),
            None => hit_block(host, level, &shot, event.block, event.face),
        };
        if !used {
            // `recoverItem`, or the type's drop (`dropStack`).
            let random = level
                .with_random(&mut *host, |r| {
                    use mod_api::RandomSource;
                    r.next_float()
                })
                .unwrap_or(1.0);
            let back = if random <= shot.recovery {
                Some(shot.item.as_str())
            } else {
                shot.kind.drop
            };
            if let Some(stack) = back.and_then(|i| stack_of(&*host, i)) {
                let _ = level.spawn_item(host, event.at[0], event.at[1], event.at[2], stack, 10);
            }
        }
        // The snowball's own hit (no damage of its own) breaks it.
        Flow::Continue
    }
}

fn stack_of(host: &dyn mod_api::Host, id: &str) -> Option<ItemStack> {
    ItemStack::new(host.item_key(&Identifier::parse(id).ok()?)?, 1)
}

/// The type's action, as `potato::Type::action` names it: its word and the rest.
fn action(kind: &Type) -> Option<(&'static str, Vec<&'static str>)> {
    let mut words = kind.action?.split(' ');
    let first = words.next()?;
    Some((first, words.collect()))
}

/// A shot hitting `target`: whether its item was used up (nothing comes back).
fn hit_entity(
    host: &mut dyn ServerHost,
    shot: &Shot,
    projectile: Option<EntityRef>,
    target: EntityRef,
) -> bool {
    let (damage, knockback, invulnerable) = potato::hit(shot.kind);
    let hurt = target
        .hurt(host, "minecraft:thrown", damage, projectile, shot.owner)
        .unwrap_or(false);
    if !hurt {
        return true;
    }
    let used = match action(shot.kind) {
        Some(("set_on_fire", a)) => {
            let ticks = a.first().and_then(|t| t.parse().ok()).unwrap_or(0);
            let _ = target.set_remaining_fire_ticks(host, ticks);
            false
        }
        Some(("potion_effect", a)) => {
            if let (Some(effect), Some(level), Some(ticks)) = (
                a.first().and_then(|e| host.effect_key(e)),
                a.get(1).and_then(|l| l.parse::<i32>().ok()),
                a.get(2).and_then(|t| t.parse::<i32>().ok()),
            ) && let Some(living) = host.living(target)
            {
                let _ = living.add_effect(
                    host,
                    &ActiveEffect::new(effect, ticks, level - 1, false, true, true),
                );
            }
            true
        }
        _ => false,
    };
    let Some(living) = host.living(target) else {
        return used;
    };
    if let Some(ticks) = invulnerable {
        let _ = target.set_invulnerable_time(host, ticks);
    }
    knock_back(host, living, target, projectile, knockback);
    used
}

/// `LivingEntity.knockback(knockback * .6, -dx, -dz)` along the shot's horizontal motion.
fn knock_back(
    host: &mut dyn ServerHost,
    _living: LivingRef,
    target: EntityRef,
    projectile: Option<EntityRef>,
    knockback: f32,
) {
    if knockback <= 0.0 {
        return;
    }
    let Some(m) = projectile.and_then(|p| p.velocity(&*host)) else {
        return;
    };
    let l = (m[0] * m[0] + m[2] * m[2]).sqrt();
    if l < 1e-4 {
        return;
    }
    let strength = f64::from(knockback) * 0.6;
    let (dx, dz) = (m[0] / l * strength, m[2] / l * strength);
    let v = target.velocity(&*host).unwrap_or([0.0; 3]);
    let on_ground = target.on_ground(&*host).unwrap_or(false);
    let y = if on_ground {
        (v[1] / 2.0 + strength).min(0.4)
    } else {
        v[1]
    };
    let _ = target.set_velocity(host, [v[0] / 2.0 + dx, y, v[2] / 2.0 + dz]);
}

/// A shot hitting the block at `pos` on `face`: whether its item was used up (a crop planted on
/// farmland, a block put on the ground).
fn hit_block(
    host: &mut dyn ServerHost,
    level: mod_api::LevelRef,
    shot: &Shot,
    pos: [i32; 3],
    face: [i32; 3],
) -> bool {
    let Some((word, a)) = action(shot.kind) else {
        return false;
    };
    let place = [pos[0] + face[0], pos[1] + face[1], pos[2] + face[2]];
    let replaceable = level
        .block_state(&*host, place)
        .and_then(|s| mod_api::HostBlockStates::block_states(&*host).replaceable(s))
        .unwrap_or(false);
    let block = |host: &dyn mod_api::Host, id: &str| {
        let key = host.block_key(id)?;
        mod_api::BlockState::of(&mod_api::HostBlockStates::block_states(host), key, &[]).ok()
    };
    match word {
        // `PlantCrop`: on top of farmland.
        "plant_crop" if face == [0, 1, 0] && replaceable => {
            let on = level
                .block_state(&*host, pos)
                .and_then(|s| host.block_state_string(s))
                .is_some_and(|s| states::split(&s).0 == "minecraft:farmland");
            let Some(crop) = a.first().and_then(|c| block(&*host, c)) else {
                return false;
            };
            if !on {
                return false;
            }
            let _ = level.set_block(host, place, crop, super::super::server::update_all());
            true
        }
        // `PlaceBlockOnGround`: on top of what it hit (a falling block from a side is left out).
        "place_block_on_ground" if face == [0, 1, 0] && replaceable => {
            let Some(b) = a.first().and_then(|c| block(&*host, c)) else {
                return false;
            };
            let _ = level.set_block(host, place, b, super::super::server::update_all());
            true
        }
        _ => false,
    }
}
