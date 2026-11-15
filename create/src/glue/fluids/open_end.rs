//! What an open pipe end lets out does to what is in front of it (`OpenPipeEffectHandler`,
//! `create_engine::open_end`): water puts out burning entities, fire, lit candles and
//! campfires; lava sets entities burning; milk clears effects; builder's tea gives Haste; a
//! potion gives its effects, its contents' custom ones too.

use super::super::server::ServerLevel;
use super::{all, property, state_with};
use create_engine::open_end::{self, Effect};
use create_engine::tank::Fluid;
use mod_api::{ActiveEffect, EntityRef, HostBlockStates, SoundSource, Tag};

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
            // Without particles or icon.
            let haste = Given {
                id: "minecraft:haste".into(),
                amplifier: 0,
                ticks: 21,
                ambient: false,
                particles: false,
                icon: false,
            };
            for e in affected(lv, &entities) {
                give(lv, e, &haste);
            }
        }
        Effect::Potion => {
            let Fluid::Potion(p) = fluid else {
                return;
            };
            let effects = potion_effects(p.contents);
            if effects.is_empty() {
                return;
            }
            for e in affected(lv, &entities) {
                let inverted = e.kind(&*lv.host).is_some_and(|k| {
                    lv.host
                        .entity_type_in_tag(&k, "minecraft:inverted_healing_and_harm")
                });
                for given in &effects {
                    match open_end::instant(&given.id, given.amplifier, inverted) {
                        Some(h) if h > 0.0 => {
                            if let Some(living) = lv.host.living(e) {
                                living.heal(&mut *lv.host, h);
                            }
                        }
                        Some(h) => {
                            let _ = e.hurt(&mut *lv.host, "minecraft:magic", -h, None, None);
                        }
                        None => give(lv, e, given),
                    }
                }
            }
        }
    }
}

/// A `MobEffectInstance` to give: effect id, amplifier, ticks, ambient, with its particles,
/// with its icon.
struct Given {
    id: String,
    amplifier: i32,
    ticks: i32,
    ambient: bool,
    particles: bool,
    icon: bool,
}

/// A potion's effects (`PotionContents.forEachEffect`): its registered potion's, then its custom
/// ones as saved (`MobEffectInstance`'s codec: id, amplifier, duration, ambient, show_particles,
/// show_icon).
fn potion_effects(c: &create_engine::potion::Contents) -> Vec<Given> {
    let mut out: Vec<Given> = c
        .potion
        .as_deref()
        .map_or(&[][..], open_end::potion_effects)
        .iter()
        .map(|&(e, amplifier, ticks)| Given {
            id: format!("minecraft:{e}"),
            amplifier,
            ticks,
            ambient: false,
            particles: true,
            icon: true,
        })
        .collect();
    let Ok((Tag::List(custom), _)) = Tag::from_network_bytes(&c.custom_effects, None) else {
        return out;
    };
    let int = |t: Option<&Tag>, or: i32| match t {
        Some(Tag::Byte(v)) => i32::from(*v),
        Some(Tag::Short(v)) => i32::from(*v),
        Some(Tag::Int(v)) => *v,
        _ => or,
    };
    for e in custom {
        let Tag::Compound(e) = e else { continue };
        let Some(Tag::String(id)) = e.get(&"id".into()) else {
            continue;
        };
        let particles = int(e.get(&"show_particles".into()), 1) != 0;
        out.push(Given {
            id: id.to_string_lossy(),
            amplifier: int(e.get(&"amplifier".into()), 0),
            ticks: int(e.get(&"duration".into()), 0),
            ambient: int(e.get(&"ambient".into()), 0) != 0,
            particles,
            icon: int(e.get(&"show_icon".into()), i32::from(particles)) != 0,
        });
    }
    out
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

/// `new MobEffectInstance(..)` of `given` on `e`.
fn give(lv: &mut ServerLevel<'_>, e: EntityRef, given: &Given) {
    let (Some(living), Some(key)) = (lv.host.living(e), lv.host.effect_key(&given.id)) else {
        return;
    };
    let effect = ActiveEffect::new(
        key,
        given.ticks,
        given.amplifier,
        given.ambient,
        given.particles,
        given.icon,
    );
    living.add_effect(&mut *lv.host, &effect);
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
