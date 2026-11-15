//! The cardboard sword: it bonks instead of hurting (`CardboardSwordItem`), knocking the target
//! back as a blow would, slowing creatures it hits (`MobCategory.CREATURE`/`MISC`, players not)
//! and the attacker, as hard as the attacker's `ATTACK_KNOCKBACK` + 2, the sword's Knockback
//! level and a sprint make it, less the target's knockback resistance. Left out: stopping the
//! attacker's sprint (no API sets it; `API_GAPS.md`). Its click on a block
//! (`cardboardSwordsMakeNoiseOnClick`: the clicking client plays it, the server plays it to the
//! others) is played here from the server to every player near it, the clicker included: the
//! API has no sound at a position that leaves one player out (API_GAPS.md).

use super::{Item, held, is};
use mod_api::{
    ActiveEffect, AttackEntityEvent, Flow, Hand, LeftClickBlockEvent, MobCategory, ServerHost,
    SoundSource,
};

/// `EntityTypeTags.ARTHROPOD` (vanilla 26.3's `entity_type/arthropod`).
const ARTHROPODS: &str = "minecraft:arthropod";

/// A left click on a block with the sword in the main hand: the bonk.
pub(super) fn left_click(host: &mut dyn ServerHost, event: &LeftClickBlockEvent) {
    let Some(player) = host.players().acting_player() else {
        return;
    };
    if !is(
        held(&*host, player, Hand::Main).as_ref(),
        Item::CardboardSword,
    ) {
        return;
    }
    let Some(level) = player.level(&*host) else {
        return;
    };
    let at = event.pos.map(|v| f64::from(v) + 0.5);
    level.play_sound(
        host,
        "create:cardboard_bonk",
        at,
        SoundSource::Players,
        0.5,
        1.85,
    );
}

/// `CardboardSwordItem.cardboardSwordsCannotHurtYou`: a cardboard sword in the main hand bonks a
/// living target other than an arthropod instead of hurting it.
pub(super) fn attack(host: &mut dyn ServerHost, event: &AttackEntityEvent<'_>) -> Flow {
    let Some(player) = host.players().acting_player() else {
        return Flow::Continue;
    };
    if !is(
        held(&*host, player, Hand::Main).as_ref(),
        Item::CardboardSword,
    ) || host.entity_type_in_tag(event.kind, ARTHROPODS)
    {
        return Flow::Continue;
    }
    let Some(target) = u32::try_from(event.entity)
        .ok()
        .and_then(|id| host.entity(id))
        .filter(|e| host.living(*e).is_some())
    else {
        return Flow::Continue;
    };
    if let (Some(level), Some(at)) = (player.level(&*host), player.entity().position(&*host)) {
        level.play_sound(
            host,
            "create:cardboard_bonk",
            at,
            SoundSource::Players,
            0.75,
            1.85,
        );
    }
    knock_back(host, player, target);
    if event.kind != "minecraft:player" {
        slow_creature(host, target);
    }
    Flow::Cancel
}

/// `MobEffectInstance(MOVEMENT_SLOWDOWN, 60, 9, true, false, false)` on a creature or a
/// `MISC` mob the sword hits (players are left out by the caller).
fn slow_creature(host: &mut dyn ServerHost, target: mod_api::EntityRef) {
    if !matches!(
        target.classification(&*host),
        Some(MobCategory::Creature | MobCategory::Misc)
    ) {
        return;
    }
    let (Some(living), Some(key)) = (host.living(target), host.effect_key("minecraft:slowness"))
    else {
        return;
    };
    living.add_effect(host, &ActiveEffect::new(key, 60, 9, true, false, false));
}

/// The knockback of `player.attack()`, without the hurt: `ATTACK_KNOCKBACK` (0) + 2, one more
/// for a sprinting blow, through `LivingEntity.knockback(strength / 2, sin yaw, -cos yaw)`;
/// then the attacker's own motion is cut to 0.6 across.
fn knock_back(host: &mut dyn ServerHost, player: mod_api::PlayerRef, target: mod_api::EntityRef) {
    let attacker = player.entity();
    target.stop_riding(host);
    let yaw = attacker.rotation(&*host).map_or(0.0, |r| r[0]).to_radians();
    // A sprinting blow, charged past 0.9 (`getAttackStrengthScale(0.5f)`).
    let sprinting = attacker.is_sprinting(&*host).unwrap_or(false)
        && player.attack_strength_scale(&*host, 0.5).unwrap_or(1.0) > 0.9;
    // `ATTACK_KNOCKBACK + 2`, `EnchantmentHelper.modifyKnockback` (vanilla's Knockback: one per
    // level), one more sprinting.
    let base = host
        .living(attacker)
        .and_then(|l| l.attribute_value(&*host, "minecraft:attack_knockback"))
        .unwrap_or(0.0)
        + 2.0;
    let enchanted = held(&*host, player, Hand::Main)
        .and_then(|s| super::filters::component(&*host, &s, "minecraft:enchantments"))
        .and_then(|t| match t {
            mod_api::Tag::Compound(c) => {
                let map = match c.get(&"levels".into()) {
                    Some(mod_api::Tag::Compound(l)) => l.clone(),
                    _ => c,
                };
                match map.get(&"minecraft:knockback".into()) {
                    Some(mod_api::Tag::Int(i)) => Some(f64::from(*i)),
                    _ => None,
                }
            }
            _ => None,
        })
        .unwrap_or(0.0);
    let knockback = base + enchanted + if sprinting { 1.0 } else { 0.0 };
    if knockback <= 0.0 {
        return;
    }
    // `LivingEntity.knockback`: less the target's resistance.
    let resistance = host
        .living(target)
        .and_then(|l| l.attribute_value(&*host, "minecraft:knockback_resistance"))
        .unwrap_or(0.0);
    let strength = knockback * 0.5 * (1.0 - resistance);
    if strength <= 0.0 {
        if let Some(v) = attacker.velocity(&*host) {
            attacker.set_velocity(host, [v[0] * 0.6, v[1], v[2] * 0.6]);
        }
        return;
    }
    let (x, z) = (f64::from(yaw.sin()), f64::from(-yaw.cos()));
    let len = (x * x + z * z).sqrt().max(1e-9);
    let push = [x / len * strength, z / len * strength];
    if let Some(old) = target.velocity(&*host) {
        let on_ground = target.on_ground(&*host).unwrap_or(false);
        let y = if on_ground {
            (old[1] / 2.0 + strength).min(0.4)
        } else {
            old[1]
        };
        target.set_velocity(host, [old[0] / 2.0 - push[0], y, old[2] / 2.0 - push[1]]);
    }
    if let Some(v) = attacker.velocity(&*host) {
        attacker.set_velocity(host, [v[0] * 0.6, v[1], v[2] * 0.6]);
    }
}
