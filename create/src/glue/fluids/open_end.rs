//! What an open pipe end lets out does to what is in front of it (`OpenPipeEffectHandler`,
//! `create_engine::open_end`): water puts out burning entities, fire, lit candles and
//! campfires; lava sets entities burning; milk clears effects; builder's tea gives Haste; a
//! potion gives its effects.

use super::super::server::ServerLevel;
use super::{all, property, state_with};
use create_engine::open_end::{self, Effect};
use create_engine::tank::Fluid;
use mod_api::{ActiveEffect, EntityRef, HostBlockStates, SoundSource};

/// `OpenPipeEffectHandler.apply` of `fluid` let out at `out` by a pipe pointing `down` or not.
pub(super) fn apply(lv: &mut ServerLevel<'_>, out: [i32; 3], down: bool, fluid: Fluid) {
    let Some(effect) = open_end::effect(fluid) else {
        return;
    };
    if !effect.acts_at(lv.game_time()) {
        return;
    }
    let (min, max) = open_end::area(out, down);
    let entities = lv
        .level
        .entities_in(&*lv.host, min, max)
        .unwrap_or_default();
    match effect {
        Effect::Extinguish => {
            for e in entities {
                if e.fire_ticks(&*lv.host).is_some_and(|t| t > 0) {
                    e.clear_fire(&mut *lv.host);
                }
            }
            let [x0, y0, z0] = min.map(|v| v as i32);
            let [x1, y1, z1] = max.map(|v| v as i32);
            for x in x0..=x1 {
                for y in y0..=y1 {
                    for z in z0..=z1 {
                        dowse_fire(lv, [x, y, z]);
                    }
                }
            }
        }
        Effect::Ignite => {
            for e in entities {
                if e.fire_immune(&*lv.host) == Some(false) {
                    // `igniteForSeconds(3)`: at least 60 ticks.
                    let ticks = e.fire_ticks(&*lv.host).unwrap_or(0);
                    if ticks < 60 {
                        e.set_remaining_fire_ticks(&mut *lv.host, 60);
                    }
                }
            }
        }
        Effect::Cure => {
            for e in affected(lv, &entities) {
                if let Some(living) = lv.host.living(e) {
                    living.remove_all_effects(&mut *lv.host);
                }
            }
        }
        Effect::Haste => {
            for e in affected(lv, &entities) {
                give(lv, e, "minecraft:haste", 21, 0, false);
            }
        }
        Effect::Potion => {
            let Fluid::Potion(p) = fluid else {
                return;
            };
            let effects = open_end::potion_effects(p.id());
            if effects.is_empty() {
                return;
            }
            for e in affected(lv, &entities) {
                let inverted = e.kind(&*lv.host).is_some_and(|k| {
                    lv.host
                        .entity_type_in_tag(&k, "minecraft:inverted_healing_and_harm")
                });
                for &(id, ticks, amplifier) in effects {
                    let id = format!("minecraft:{id}");
                    match open_end::instant(&id, amplifier, inverted) {
                        Some(h) if h > 0.0 => {
                            if let Some(living) = lv.host.living(e) {
                                living.heal(&mut *lv.host, h);
                            }
                        }
                        Some(h) => {
                            let _ = e.hurt(&mut *lv.host, "minecraft:magic", -h, None, None);
                        }
                        None => give(lv, e, &id, ticks, amplifier, true),
                    }
                }
            }
        }
    }
}

/// The living entities of `entities` potions act on (`LivingEntity::isAffectedByPotions`: alive,
/// and not an armor stand or a spectator).
fn affected(lv: &ServerLevel<'_>, entities: &[EntityRef]) -> Vec<EntityRef> {
    let spectators: Vec<EntityRef> = lv
        .level
        .online_players(&*lv.host)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.is_spectator(&*lv.host) == Some(true))
        .map(|p| p.entity())
        .collect();
    entities
        .iter()
        .copied()
        .filter(|e| {
            let Some(living) = lv.host.living(*e) else {
                return false;
            };
            if living.alive(&*lv.host) != Some(true) {
                return false;
            }
            if e.kind(&*lv.host).as_deref() == Some("minecraft:armor_stand") {
                return false;
            }
            !spectators.contains(e)
        })
        .collect()
}

/// `new MobEffectInstance(effect, ticks, amplifier)` on `e`; `shown`: with its particles and
/// icon (a potion's), else neither (builder's tea's).
fn give(lv: &mut ServerLevel<'_>, e: EntityRef, effect: &str, ticks: i32, amp: i32, shown: bool) {
    let (Some(living), Some(key)) = (lv.host.living(e), lv.host.effect_key(effect)) else {
        return;
    };
    living.add_effect(
        &mut *lv.host,
        &ActiveEffect::new(key, ticks, amp, false, shown, shown),
    );
}

/// `WaterEffectHandler.dowseFire` at `p`: fire goes, a lit candle or campfire goes out.
fn dowse_fire(lv: &mut ServerLevel<'_>, p: [i32; 3]) {
    let Some(state) = lv.block_state(p) else {
        return;
    };
    let in_tag = |tag: &str| lv.host.block_in_tag(state.block(), tag) == Some(true);
    if in_tag("minecraft:fire") {
        if let Some(air) = state_with(lv, "minecraft:air", &[]) {
            lv.set_state(p, air, all());
        }
        return;
    }
    let lit = property(lv, p, "lit").as_deref() == Some("true");
    if !lit {
        return;
    }
    let meta = lv.host.block_states();
    let Ok(out) = state.with(&meta, "lit", "false") else {
        return;
    };
    if in_tag("minecraft:candles") || in_tag("minecraft:candle_cakes") {
        // `AbstractCandleBlock.extinguish`.
        let at = p.map(|c| f64::from(c) + 0.5);
        lv.level.play_sound(
            &mut *lv.host,
            "minecraft:block.candle.extinguish",
            at,
            SoundSource::Blocks,
            1.0,
            1.0,
        );
        lv.set_state(p, out, all());
    } else if in_tag("minecraft:campfires") {
        // `SOUND_EXTINGUISH_FIRE`, `CampfireBlock.dowse`.
        lv.level.level_event(&mut *lv.host, 1009, p, 0);
        lv.set_state(p, out, all());
    }
}
