//! What an open pipe end does with the fluid it lets out (`OpenEndedPipe`,
//! `AllOpenPipeEffectHandlers`): water, lava, honey and chocolate become a source block a bucket's
//! worth at a time; milk, builder's tea and potions have no block, so the end takes a drop a fill
//! and gives what is in front of it their effect (`OpenPipeEffectHandler`); a fluid let into
//! another fluid's block turns it as `FluidReactions.handlePipeSpillCollision` says.
//!
//! Each open end keeps a tank of its own (`OpenEndFluidHandler`, a `FluidTank` of a bucket):
//! what a pump drew from a source block beyond what it moved, and what it let out until a
//! bucket's worth becomes a source in front (`OpenEnd`). Water let out where water boils away
//! (the Nether) is lost with a hiss; water fills and drains a waterloggable block in front.

use crate::tank::{BUCKET, Fluid};

/// The world in front of an open end, as `OpenEndedPipe` reads and changes it.
pub trait Space {
    /// `VanillaFluidTargets.drainBlock`: a full beehive's honey (250), a lava cauldron's or a
    /// full water cauldron's bucket, emptied unless `simulate`.
    fn drain_block(&mut self, simulate: bool) -> Option<(Fluid, u32)>;
    /// The block has a `waterlogged` property.
    fn waterloggable(&self) -> bool;
    /// `BlockState.canBeReplaced`.
    fn replaceable(&self) -> bool;
    /// The fluid in the block (`getFluidState`): none when empty; else the fluid (none for one
    /// the port does not know) and whether it is a source.
    fn fluid(&self) -> Option<(Option<Fluid>, bool)>;
    /// `dimensionType().ultraWarm()`: water placed here boils away.
    fn evaporates(&self) -> bool;
    /// `FluidReactions.handlePipeSpillCollision` of `pipe` let into `world` (a source or not).
    fn spill(&mut self, pipe: Fluid, world: Option<Fluid>, source: bool);
    /// `WATERLOGGED` set to `on`, and a water tick scheduled a tick later.
    fn set_waterlogged(&mut self, on: bool);
    /// `fluid`'s source block placed.
    fn place_source(&mut self, fluid: Fluid);
    /// The source block taken: its fluid's block at level 14, unless the spot would at once
    /// become the same source again (`getNewLiquid`: water between two sources over a solid
    /// block or a source).
    fn take_source(&mut self);
    /// A source of `fluid` was taken out of the world into the pipe (`WATER_SUPPLY`).
    fn took(&mut self, fluid: Fluid) {
        let _ = fluid;
    }
    /// `FIRE_EXTINGUISH` at 0.5 and a random high pitch: water boiled away.
    fn fizz(&mut self);
    /// `OpenPipeEffectHandler.apply` of `fluid` on what is in front.
    fn effect(&mut self, fluid: Fluid);
}

/// An open end's own tank (`OpenEndFluidHandler`, capacity a bucket) and whether a pump last
/// drew from it (`wasPulling`, saved).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenEnd {
    pub tank: Option<(Fluid, u32)>,
    pub pulling: bool,
}

impl OpenEnd {
    /// `OpenEndFluidHandler.fill` of `amount` of `fluid` (`execute`, or simulated): how much it
    /// took. A fluid with an effect and no block (milk, tea, potions) is taken a drop at a time,
    /// as is the first drop into an empty end of a fluid with an effect (water, lava: Create
    /// asks whether what the end *held* has a block); a full bucket's worth (or any fluid with
    /// no block) goes out into the world.
    pub fn fill(&mut self, s: &mut impl Space, fluid: Fluid, amount: u32, execute: bool) -> u32 {
        if amount == 0 || !provide(s, fluid, true) {
            return 0;
        }
        let contained = self.tank;
        let has_block = contained.is_some_and(|(f, _)| f.placeable());
        if contained.is_some_and(|(f, _)| f != fluid) {
            self.tank = None;
        }
        self.pulling = false;
        let handler = effect(fluid).is_some();
        let amount = if handler && !has_block { 1 } else { amount };
        // `FluidTank.fill`.
        let have = self.tank.map_or(0, |t| t.1);
        let filled = amount.min(BUCKET - have);
        if !execute {
            return filled;
        }
        if filled > 0 {
            self.tank = Some((fluid, have + filled));
        }
        if handler {
            s.effect(fluid);
        }
        let held = self.tank.map_or(0, |t| t.1);
        if held == BUCKET || !has_block {
            // `containedFluidStack`: the tank's own stack when it was this fluid (grown by the
            // fill), else what it held before (none, or the other fluid it lost).
            let offered = match contained {
                Some((f, _)) if f == fluid => self.tank,
                other => other,
            };
            if let Some((f, _)) = offered
                && provide(s, f, false)
            {
                self.tank = None;
            }
        }
        filled
    }

    /// `OpenEndFluidHandler.drainInner` of up to `amount` (at most a bucket), of `only` or any
    /// fluid (`execute`, or simulated): from its tank, else a source block in front (a bucket's
    /// worth, what the pump does not take kept in its tank).
    pub fn drain(
        &mut self,
        s: &mut impl Space,
        amount: u32,
        only: Option<Fluid>,
        execute: bool,
    ) -> Option<(Fluid, u32)> {
        if amount == 0 {
            return None;
        }
        let amount = amount.min(BUCKET);
        self.pulling = true;
        // `FluidTank.drain`.
        if let Some((f, have)) = self.tank
            && only.is_none_or(|o| o == f)
        {
            let n = amount.min(have);
            if execute {
                self.tank = (have > n).then_some((f, have - n));
            }
            return Some((f, n));
        }
        let (f, got) = remove_from_space(s, !execute)?;
        if only.is_some_and(|o| o != f) {
            return None;
        }
        if execute && got > amount {
            if self.tank.is_some_and(|(t, _)| t != f) {
                self.tank = None;
            }
            let have = self.tank.map_or(0, |t| t.1);
            let rest = (got - amount).min(BUCKET - have);
            if rest > 0 {
                self.tank = Some((f, have + rest));
            }
        }
        // `drainedFromWorld.setAmount(amount)`, even past what the block gave.
        Some((f, amount))
    }
}

/// `OpenEndedPipe.provideFluidToSpace` of `fluid` (`simulate`, or for real): whether the space
/// takes it.
fn provide(s: &mut impl Space, fluid: Fluid, simulate: bool) -> bool {
    let waterlog = s.waterloggable();
    if !waterlog && !s.replaceable() {
        return false;
    }
    // A fluid with no block goes into thin air.
    if !fluid.placeable() {
        return true;
    }
    let there = s.fluid();
    if let Some((f, source)) = there
        && f != Some(fluid)
    {
        s.spill(fluid, f, source);
        return false;
    }
    if there.is_some_and(|(_, source)| source) {
        return false;
    }
    if waterlog && fluid != Fluid::Water {
        return false;
    }
    if simulate {
        return true;
    }
    if s.evaporates() && fluid == Fluid::Water {
        s.fizz();
        return true;
    }
    if waterlog {
        s.set_waterlogged(true);
        return true;
    }
    s.place_source(fluid);
    true
}

/// `OpenEndedPipe.removeFluidFromSpace` (`simulate`, or for real): a bucket's worth of the
/// source in front (or what a cauldron or beehive gives).
fn remove_from_space(s: &mut impl Space, simulate: bool) -> Option<(Fluid, u32)> {
    if let Some(d) = s.drain_block(simulate) {
        return Some(d);
    }
    let waterlog = s.waterloggable();
    if !waterlog && !s.replaceable() {
        return None;
    }
    let (Some(fluid), true) = s.fluid()? else {
        return None;
    };
    if !simulate {
        s.took(fluid);
        if waterlog {
            s.set_waterlogged(false);
        } else {
            s.take_source();
        }
    }
    Some((fluid, BUCKET))
}

/// `handlePipeFlowCollisionFallback`: the block a pipe where flows of `a` and `b` meet becomes
/// once broken (none: it is only broken).
pub fn flow_collision(a: Fluid, b: Fluid) -> Option<&'static str> {
    let lava_with = |f: Fluid| match f {
        Fluid::Honey => Some("create:limestone"),
        Fluid::Chocolate => Some("create:scoria"),
        _ => None,
    };
    match (a, b) {
        (Fluid::Water, Fluid::Lava) | (Fluid::Lava, Fluid::Water) => Some("minecraft:cobblestone"),
        (Fluid::Lava, f) | (f, Fluid::Lava) if f.placeable() => lava_with(f),
        _ => None,
    }
}

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
    } else if world == Fluid::Lava
        && !source
        && pipe.placeable()
        && let Some(s) = lava_with(pipe)
    {
        state = Some(s);
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
    instant_at(effect, amplifier, inverted, 0.5)
}

/// `HealOrHarmMobEffect.applyInstantenousEffect` at `health` (1 for a potion drunk,
/// `PotionContents.applyToLivingEntity`; 0.5 for a fluid's splash).
pub fn instant_at(effect: &str, amplifier: i32, inverted: bool, health: f64) -> Option<f32> {
    let harm = match effect.strip_prefix("minecraft:").unwrap_or(effect) {
        "instant_health" => false,
        "instant_damage" => true,
        _ => return None,
    };
    let amp = amplifier.clamp(0, 30);
    if harm == inverted {
        Some((health * f64::from(4 << amp) + 0.5) as i32 as f32)
    } else {
        Some(-((health * f64::from(6 << amp) + 0.5) as i32 as f32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_potions_have_their_effects() {
        assert_eq!(potion_effects("minecraft:swiftness"), &[("speed", 0, 3600)]);
        assert_eq!(
            potion_effects("strong_healing"),
            &[("instant_health", 1, 1)]
        );
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
        assert_eq!(
            spill(Fluid::Water, Fluid::Lava, true),
            Some("minecraft:obsidian")
        );
        assert_eq!(
            spill(Fluid::Water, Fluid::Lava, false),
            Some("minecraft:cobblestone")
        );
        assert_eq!(
            spill(Fluid::Lava, Fluid::Water, true),
            Some("minecraft:stone")
        );
        assert_eq!(
            spill(Fluid::Lava, Fluid::Water, false),
            Some("minecraft:cobblestone")
        );
        assert_eq!(
            spill(Fluid::Lava, Fluid::Honey, true),
            Some("create:limestone")
        );
        assert_eq!(
            spill(Fluid::Chocolate, Fluid::Lava, false),
            Some("create:scoria")
        );
        assert_eq!(spill(Fluid::Chocolate, Fluid::Lava, true), None);
        assert_eq!(spill(Fluid::Honey, Fluid::Water, true), None);
    }

    /// A block in front of an open end.
    #[derive(Default)]
    struct Spot {
        air: bool,
        waterlogged: Option<bool>,
        fluid: Option<(Option<Fluid>, bool)>,
        nether: bool,
        cauldron: Option<(Fluid, u32)>,
        log: Vec<String>,
    }

    impl Space for Spot {
        fn drain_block(&mut self, simulate: bool) -> Option<(Fluid, u32)> {
            let got = self.cauldron?;
            if !simulate {
                self.cauldron = None;
            }
            Some(got)
        }
        fn waterloggable(&self) -> bool {
            self.waterlogged.is_some()
        }
        fn replaceable(&self) -> bool {
            self.air
        }
        fn fluid(&self) -> Option<(Option<Fluid>, bool)> {
            match self.waterlogged {
                Some(true) => Some((Some(Fluid::Water), true)),
                Some(false) => None,
                None => self.fluid,
            }
        }
        fn evaporates(&self) -> bool {
            self.nether
        }
        fn spill(&mut self, pipe: Fluid, world: Option<Fluid>, source: bool) {
            self.log.push(format!("spill {pipe:?} {world:?} {source}"));
        }
        fn set_waterlogged(&mut self, on: bool) {
            self.waterlogged = Some(on);
            self.log.push(format!("waterlogged {on}"));
        }
        fn place_source(&mut self, fluid: Fluid) {
            self.fluid = Some((Some(fluid), true));
            self.log.push(format!("source {fluid:?}"));
        }
        fn take_source(&mut self) {
            self.fluid = Some((self.fluid.and_then(|f| f.0), false));
            self.log.push("taken".into());
        }
        fn fizz(&mut self) {
            self.log.push("fizz".into());
        }
        fn effect(&mut self, fluid: Fluid) {
            self.log.push(format!("effect {fluid:?}"));
        }
    }

    fn air() -> Spot {
        Spot {
            air: true,
            ..Spot::default()
        }
    }

    #[test]
    fn an_open_end_lets_out_a_bucket_as_a_source_with_the_first_drop_alone() {
        let mut end = OpenEnd::default();
        let mut s = air();
        // Water has an effect and the end held nothing: one drop only.
        assert_eq!(end.fill(&mut s, Fluid::Water, 40, true), 1);
        assert_eq!(end.tank, Some((Fluid::Water, 1)));
        assert_eq!(end.fill(&mut s, Fluid::Water, 40, true), 40);
        assert_eq!(end.tank, Some((Fluid::Water, 41)));
        // Honey has no effect: all of it.
        let mut honey = OpenEnd::default();
        assert_eq!(honey.fill(&mut s, Fluid::Honey, 40, true), 40);
        // A simulated fill changes nothing.
        assert_eq!(end.fill(&mut s, Fluid::Water, 2000, false), 959);
        assert_eq!(end.tank, Some((Fluid::Water, 41)));
        assert_eq!(end.fill(&mut s, Fluid::Water, 2000, true), 959);
        assert_eq!(end.tank, None);
        assert_eq!(s.fluid, Some((Some(Fluid::Water), true)));
        // A source in front takes no more.
        assert_eq!(end.fill(&mut s, Fluid::Water, 40, true), 0);
    }

    #[test]
    fn two_open_ends_keep_their_own_tanks() {
        let (mut a, mut b) = (OpenEnd::default(), OpenEnd::default());
        let (mut sa, mut sb) = (air(), air());
        a.fill(&mut sa, Fluid::Lava, 1, true);
        a.fill(&mut sa, Fluid::Lava, 500, true);
        b.fill(&mut sb, Fluid::Lava, 1, true);
        b.fill(&mut sb, Fluid::Lava, 300, true);
        assert_eq!(a.tank, Some((Fluid::Lava, 501)));
        assert_eq!(b.tank, Some((Fluid::Lava, 301)));
    }

    #[test]
    fn milk_is_let_out_a_drop_at_a_time_for_its_effect() {
        let mut end = OpenEnd::default();
        let mut s = air();
        assert_eq!(end.fill(&mut s, Fluid::Milk, 40, true), 1);
        assert_eq!(end.fill(&mut s, Fluid::Milk, 40, true), 1);
        assert_eq!(end.tank, None);
        assert_eq!(s.log, ["effect Milk", "effect Milk"]);
        assert_eq!(s.fluid, None);
    }

    #[test]
    fn water_boils_away_in_the_nether() {
        let mut end = OpenEnd {
            tank: Some((Fluid::Water, 999)),
            pulling: false,
        };
        let mut s = Spot {
            nether: true,
            ..air()
        };
        assert_eq!(end.fill(&mut s, Fluid::Water, 1, true), 1);
        assert_eq!(end.tank, None);
        assert_eq!(s.fluid, None);
        assert_eq!(s.log, ["effect Water", "fizz"]);
        // Lava is placed there all the same.
        let mut lava = OpenEnd {
            tank: Some((Fluid::Lava, 999)),
            pulling: false,
        };
        lava.fill(&mut s, Fluid::Lava, 1, true);
        assert_eq!(s.fluid, Some((Some(Fluid::Lava), true)));
    }

    #[test]
    fn waterloggable_blocks_take_and_give_water_only() {
        let mut slab = Spot {
            waterlogged: Some(false),
            ..Spot::default()
        };
        let mut end = OpenEnd {
            tank: Some((Fluid::Water, 999)),
            pulling: false,
        };
        assert_eq!(end.fill(&mut slab, Fluid::Water, 5, true), 1);
        assert_eq!(slab.waterlogged, Some(true));
        let mut lava = OpenEnd::default();
        assert_eq!(lava.fill(&mut slab, Fluid::Lava, 5, true), 0);
        // Drained: a bucket, what the pump does not take kept.
        let mut pull = OpenEnd::default();
        assert_eq!(
            pull.drain(&mut slab, 40, None, false),
            Some((Fluid::Water, 40))
        );
        assert_eq!(slab.waterlogged, Some(true));
        assert_eq!(
            pull.drain(&mut slab, 40, None, true),
            Some((Fluid::Water, 40))
        );
        assert_eq!(slab.waterlogged, Some(false));
        assert_eq!(pull.tank, Some((Fluid::Water, 960)));
        assert!(pull.pulling);
        assert_eq!(pull.drain(&mut slab, 40, Some(Fluid::Lava), true), None);
        assert_eq!(
            pull.drain(&mut slab, 40, Some(Fluid::Water), true),
            Some((Fluid::Water, 40))
        );
        assert_eq!(pull.tank, Some((Fluid::Water, 920)));
    }

    #[test]
    fn open_ends_drain_cauldrons_and_spill_into_other_fluids() {
        let mut s = Spot {
            cauldron: Some((Fluid::Lava, 1000)),
            ..Spot::default()
        };
        let mut end = OpenEnd::default();
        assert_eq!(end.drain(&mut s, 100, None, true), Some((Fluid::Lava, 100)));
        assert_eq!(s.cauldron, None);
        assert_eq!(end.tank, Some((Fluid::Lava, 900)));
        let mut water = Spot {
            fluid: Some((Some(Fluid::Water), false)),
            ..air()
        };
        let mut lava = OpenEnd::default();
        assert_eq!(lava.fill(&mut water, Fluid::Lava, 10, true), 0);
        assert_eq!(water.log, ["spill Lava Some(Water) false"]);
        // A flowing block of the same fluid is filled over.
        let mut flowing = Spot {
            fluid: Some((Some(Fluid::Lava), false)),
            ..air()
        };
        assert_eq!(lava.fill(&mut flowing, Fluid::Lava, 10, true), 1);
    }

    #[test]
    fn meeting_flows_break_the_pipe_into_stone() {
        assert_eq!(
            flow_collision(Fluid::Water, Fluid::Lava),
            Some("minecraft:cobblestone")
        );
        assert_eq!(
            flow_collision(Fluid::Lava, Fluid::Water),
            Some("minecraft:cobblestone")
        );
        assert_eq!(
            flow_collision(Fluid::Honey, Fluid::Lava),
            Some("create:limestone")
        );
        assert_eq!(
            flow_collision(Fluid::Lava, Fluid::Chocolate),
            Some("create:scoria")
        );
        assert_eq!(flow_collision(Fluid::Lava, Fluid::Milk), None);
        assert_eq!(flow_collision(Fluid::Water, Fluid::Honey), None);
    }

    #[test]
    fn the_area_reaches_below_and_further_when_pointing_down() {
        assert_eq!(area([0, 5, 0], false), ([0.0, 4.0, 0.0], [1.0, 6.0, 1.0]));
        assert_eq!(area([0, 5, 0], true), ([0.0, 3.0, 0.0], [1.0, 6.0, 1.0]));
    }
}
