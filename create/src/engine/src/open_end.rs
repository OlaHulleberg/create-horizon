//! What an open pipe end does with the fluid it lets out (`OpenEndedPipe`,
//! `AllOpenPipeEffectHandlers`): water, lava, honey and chocolate become a source block a bucket's
//! worth at a time; milk, builder's tea and potions have no block, so the end takes a drop a fill
//! and gives what is in front of it their effect (`OpenPipeEffectHandler`); a fluid let into
//! another fluid's block turns it as `FluidReactions.handlePipeSpillCollision` says.

use crate::tank::Fluid;

/// What the fluid does to what is in front of an open end (`OpenPipeEffectHandler.apply`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// `WaterEffectHandler`: burning entities go out, fire, lit candles and campfires too.
    Extinguish,
    /// `LavaEffectHandler`: entities that can burn burn for 3 seconds.
    Ignite,
    /// `MilkEffectHandler`: living entities lose the effects milk cures.
    Cure,
    /// `TeaEffectHandler`: living entities get Haste I for 21 ticks, without particles or icon.
    Haste,
    /// `PotionEffectHandler`: living entities get the potion's effects (an instant one at half
    /// strength), every fill.
    Potion,
}

/// The handler `OpenPipeEffectHandler.REGISTRY` has for `fluid`, if any.
pub fn effect(fluid: Fluid) -> Option<Effect> {
    Some(match fluid {
        Fluid::Water => Effect::Extinguish,
        Fluid::Lava => Effect::Ignite,
        Fluid::Milk => Effect::Cure,
        Fluid::Tea => Effect::Haste,
        Fluid::Potion(_) => Effect::Potion,
        Fluid::Honey | Fluid::Chocolate => return None,
    })
}

impl Effect {
    /// Whether it acts at game time `time`: all but potions every 5 ticks
    /// (`getGameTime() % 5 != 0` returns).
    pub fn acts_at(self, time: i64) -> bool {
        self == Effect::Potion || time % 5 == 0
    }
}

/// `OpenEndFluidHandler.fill`: how much of `offered` mB an open end takes of `fluid` with no
/// block (one: `copyStackWithAmount(resource, 1)` for a fluid with a handler); `None` for a
/// fluid with a block, which fills its bucket.
pub fn takes_without_block(fluid: Fluid, offered: u32) -> Option<u32> {
    (!fluid.placeable()).then(|| offered.min(1))
}

/// The area an open end at `out` acts on (`OpenEndedPipe.aoe`): its block and the one below,
/// and one more below when the pipe points down. Min and max corners.
pub fn area(out: [i32; 3], down: bool) -> ([f64; 3], [f64; 3]) {
    let [x, y, z] = out.map(f64::from);
    let below = if down { 2.0 } else { 1.0 };
    ([x, y - below, z], [x + 1.0, y + 1.0, z + 1.0])
}

/// `handlePipeSpillCollisionFallback`: the block a pipe letting `pipe` out into the world fluid
/// `world` (a source or flowing) puts there, if any.
pub fn spill(pipe: Fluid, world: Fluid, source: bool) -> Option<&'static str> {
    // `AllFluids.getLavaInteraction`.
    let lava_with = |f: Fluid| match f {
        Fluid::Honey => Some("create:limestone"),
        Fluid::Chocolate => Some("create:scoria"),
        _ => None,
    };
    let mut state = match (pipe, world, source) {
        (Fluid::Water, Fluid::Lava, true) => Some("minecraft:obsidian"),
        (Fluid::Water, Fluid::Lava, false) => Some("minecraft:cobblestone"),
        (Fluid::Lava, Fluid::Water, true) => Some("minecraft:stone"),
        (Fluid::Lava, Fluid::Water, false) => Some("minecraft:cobblestone"),
        _ => None,
    };
    if pipe == Fluid::Lava {
        if let Some(s) = lava_with(world) {
            state = Some(s);
        }
    } else if world == Fluid::Lava && !source && pipe.placeable() {
        if let Some(s) = lava_with(pipe) {
            state = Some(s);
        }
    }
    state
}

/// The effects of the registered potion `id` (`Potions`, [`crate::potion::POTIONS`]): effect,
/// amplifier, ticks. A potion's custom effects are its contents' own
/// (`potion::Contents::custom_effects`), which the glue reads.
pub fn potion_effects(id: &str) -> &'static [(&'static str, i32, i32)] {
    crate::potion::registered(id).map_or(&[], |i| crate::potion::POTIONS[i].2)
}

/// `HealOrHarmMobEffect.applyInstantenousEffect` at `health` 0.5 (`PotionEffectHandler`): the
/// health an instant effect of `amplifier` heals (positive) or the damage it deals (negative),
/// for an entity whose healing and harm are inverted (`inverted`: the undead) or not; none for
/// an effect that is not instant.
pub fn instant(effect: &str, amplifier: i32, inverted: bool) -> Option<f32> {
    let harm = match effect.strip_prefix("minecraft:").unwrap_or(effect) {
        "instant_health" => false,
        "instant_damage" => true,
        _ => return None,
    };
    let amp = amplifier.clamp(0, 30);
    if harm == inverted {
        Some((0.5 * f64::from(4 << amp) + 0.5) as i32 as f32)
    } else {
        Some(-((0.5 * f64::from(6 << amp) + 0.5) as i32 as f32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tank::{Bottle, Potion};

    #[test]
    fn fluids_without_a_block_are_taken_a_drop_at_a_time() {
        assert_eq!(takes_without_block(Fluid::Milk, 40), Some(1));
        assert_eq!(takes_without_block(Fluid::Tea, 40), Some(1));
        let swift = Fluid::Potion(Potion::of("minecraft:swiftness", Bottle::Regular).unwrap());
        assert_eq!(takes_without_block(swift, 40), Some(1));
        assert_eq!(takes_without_block(Fluid::Water, 40), None);
        assert_eq!(takes_without_block(Fluid::Honey, 40), None);
    }

    #[test]
    fn registered_potions_have_their_effects() {
        assert_eq!(potion_effects("minecraft:swiftness"), &[("speed", 0, 3600)]);
        assert_eq!(potion_effects("strong_healing"), &[("instant_health", 1, 1)]);
        assert_eq!(potion_effects("minecraft:awkward"), &[]);
        assert_eq!(potion_effects("minecraft:unknown"), &[]);
    }

    #[test]
    fn instant_effects_heal_and_harm_at_half_strength() {
        assert_eq!(instant("minecraft:instant_health", 0, false), Some(2.0));
        assert_eq!(instant("minecraft:instant_health", 1, false), Some(4.0));
        assert_eq!(instant("minecraft:instant_damage", 0, false), Some(-3.0));
        assert_eq!(instant("minecraft:instant_damage", 1, false), Some(-6.0));
        // The undead are hurt by healing and healed by harm.
        assert_eq!(instant("minecraft:instant_health", 0, true), Some(-3.0));
        assert_eq!(instant("minecraft:instant_damage", 0, true), Some(2.0));
        assert_eq!(instant("minecraft:speed", 0, false), None);
    }

    #[test]
    fn spills_turn_the_world_fluid_as_create_does() {
        assert_eq!(spill(Fluid::Water, Fluid::Lava, true), Some("minecraft:obsidian"));
        assert_eq!(spill(Fluid::Water, Fluid::Lava, false), Some("minecraft:cobblestone"));
        assert_eq!(spill(Fluid::Lava, Fluid::Water, true), Some("minecraft:stone"));
        assert_eq!(spill(Fluid::Lava, Fluid::Water, false), Some("minecraft:cobblestone"));
        assert_eq!(spill(Fluid::Lava, Fluid::Honey, true), Some("create:limestone"));
        assert_eq!(spill(Fluid::Chocolate, Fluid::Lava, false), Some("create:scoria"));
        assert_eq!(spill(Fluid::Chocolate, Fluid::Lava, true), None);
        assert_eq!(spill(Fluid::Honey, Fluid::Water, true), None);
    }

    #[test]
    fn the_area_reaches_below_and_further_when_pointing_down() {
        assert_eq!(area([0, 5, 0], false), ([0.0, 4.0, 0.0], [1.0, 6.0, 1.0]));
        assert_eq!(area([0, 5, 0], true), ([0.0, 3.0, 0.0], [1.0, 6.0, 1.0]));
    }
}
