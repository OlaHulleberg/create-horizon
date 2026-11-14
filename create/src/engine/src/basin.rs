//! Create's basin and the mechanical mixer over it (`BasinBlockEntity`, `BasinRecipe`,
//! `BasinOperatingBlockEntity`, `MechanicalMixerBlockEntity`, `HeatCondition`): items in the
//! basin's nine input slots, a recipe whose ingredients they hold (and whose heat the burner
//! below gives) is mixed in a cycle of the mixer's head, its results going to the nine outputs.
//! Item recipes only: fluids are gap 5.

use crate::fan::Heat;
use crate::processing::{Output, Rng, Slot, insert_stacked};
use crate::tank::Fluid;

/// `BasinBlockEntity`'s fluid tanks: two inputs and two outputs of a bucket each.
pub const TANK: u32 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tanks {
    pub input: [Option<(Fluid, u32)>; 2],
    pub output: [Option<(Fluid, u32)>; 2],
}

/// Puts up to `amount` of `fluid` in one of `tanks` (one holding it, else an empty one): what
/// went in.
pub fn fill(tanks: &mut [Option<(Fluid, u32)>; 2], fluid: Fluid, amount: u32) -> u32 {
    let i = tanks
        .iter()
        .position(|t| t.is_some_and(|(f, _)| f == fluid))
        .or_else(|| tanks.iter().position(Option::is_none));
    let Some(i) = i else {
        return 0;
    };
    let have = tanks[i].map_or(0, |t| t.1);
    let took = amount.min(TANK - have);
    if took > 0 {
        tanks[i] = Some((fluid, have + took));
    }
    took
}

/// Takes up to `amount` from the first non-empty tank: the fluid and how much.
pub fn drain(tanks: &mut [Option<(Fluid, u32)>; 2], amount: u32) -> Option<(Fluid, u32)> {
    let i = tanks.iter().position(Option::is_some)?;
    let (fluid, have) = tanks[i]?;
    let out = amount.min(have);
    tanks[i] = (have > out).then_some((fluid, have - out));
    (out > 0).then_some((fluid, out))
}

pub const INPUTS: std::ops::Range<usize> = 0..9;
pub const OUTPUTS: std::ops::Range<usize> = 9..18;
pub const SLOTS: usize = 18;

/// `HeatCondition`: a recipe's `heat_requirement`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeatReq {
    #[default]
    None,
    Heated,
    Superheated,
}

impl HeatReq {
    pub fn read(name: Option<&str>) -> HeatReq {
        match name {
            Some("heated") => HeatReq::Heated,
            Some("superheated") => HeatReq::Superheated,
            _ => HeatReq::None,
        }
    }

    /// `testBlazeBurner` against the heat below the basin (`getHeatLevelOf`).
    pub fn met(self, heat: Heat) -> bool {
        match self {
            HeatReq::Superheated => heat == Heat::Seething,
            HeatReq::Heated => heat != Heat::None && heat != Heat::Smouldering,
            HeatReq::None => true,
        }
    }
}

/// A basin recipe: one item per ingredient, its results and time, and the heat it needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub ingredients: Vec<Vec<String>>,
    pub results: Vec<Output>,
    pub time: i32,
    pub heat: HeatReq,
    /// Fluid ingredients and results (`FluidIngredient`, `FluidStack`).
    pub fluids: Vec<(Fluid, u32)>,
    pub fluid_results: Vec<(Fluid, u32)>,
}

/// `BasinRecipe.apply`: takes one item per ingredient from the inputs (each from the first slot
/// that fits), rolls the results and puts them in the outputs; nothing changes unless all of it
/// fits. With `simulate`, only says whether it would.
pub fn apply(
    slots: &mut [Slot],
    tanks: &mut Tanks,
    recipe: &Recipe,
    heat: Heat,
    max: impl Fn(&str) -> u8,
    rng: &mut Rng,
    simulate: bool,
) -> bool {
    if !recipe.heat.met(heat) {
        return false;
    }
    // Fluids: each ingredient from an input tank that holds enough; each result into the
    // outputs.
    let mut fluids = *tanks;
    for &(fluid, amount) in &recipe.fluids {
        let Some(t) = fluids
            .input
            .iter_mut()
            .find(|t| t.is_some_and(|(f, n)| f == fluid && n >= amount))
        else {
            return false;
        };
        let (f, n) = t.unwrap();
        *t = (n > amount).then_some((f, n - amount));
    }
    for &(fluid, amount) in &recipe.fluid_results {
        if fill(&mut fluids.output, fluid, amount) < amount {
            return false;
        }
    }
    let mut taken = [0u8; 9];
    for ingredient in &recipe.ingredients {
        let found = INPUTS.clone().find(|&i| {
            slots[i]
                .as_ref()
                .is_some_and(|(item, n)| *n > taken[i] && ingredient.iter().any(|x| x == item))
        });
        match found {
            Some(i) => taken[i] += 1,
            None => return false,
        }
    }
    // Results: rolled only for real (a simulation needs every one to fit at its full count).
    let results: Vec<(String, u8)> = if simulate {
        recipe
            .results
            .iter()
            .map(|o| (o.item.clone(), o.count))
            .collect()
    } else {
        crate::processing::roll(
            &crate::processing::Recipe {
                id: recipe.id.clone(),
                ingredients: Vec::new(),
                results: recipe.results.clone(),
                time: recipe.time,
            },
            rng,
        )
    };
    let mut outputs: Vec<Slot> = slots[OUTPUTS].to_vec();
    for (item, n) in &results {
        if insert_stacked(&mut outputs, item, *n, max(item)) > 0 {
            return false;
        }
    }
    if simulate {
        return true;
    }
    *tanks = fluids;
    for i in INPUTS {
        if taken[i] > 0
            && let Some((_, n)) = slots[i].as_mut()
        {
            *n -= taken[i];
            if *n == 0 {
                slots[i] = None;
            }
        }
    }
    for (slot, o) in slots[OUTPUTS].iter_mut().zip(outputs) {
        *slot = o;
    }
    true
}

/// `getMatchingRecipes`: the recipes that would apply now, those with the most ingredients first.
pub fn matching(
    recipes: &[Recipe],
    slots: &[Slot],
    tanks: &Tanks,
    heat: Heat,
    max: impl Fn(&str) -> u8 + Copy,
) -> Vec<usize> {
    let mut copy = slots.to_vec();
    let mut t = *tanks;
    let mut out: Vec<usize> = (0..recipes.len())
        .filter(|&i| {
            apply(
                &mut copy,
                &mut t,
                &recipes[i],
                heat,
                max,
                &mut Rng::new(1),
                true,
            )
        })
        .collect();
    out.sort_by_key(|&i| std::cmp::Reverse(recipes[i].ingredients.len() + recipes[i].fluids.len()));
    out
}

/// The mixer's cycle (`MechanicalMixerBlockEntity.tick`): 20 ticks down, the processing time at
/// the bottom, 20 ticks up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mixer {
    pub running: bool,
    pub running_ticks: i32,
    pub processing_ticks: i32,
    /// The recipe being mixed (`currentRecipe`), by index.
    pub recipe: Option<usize>,
}

/// What a tick of the mixer asks of the glue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixStep {
    None,
    /// At the bottom, the time is up: apply the recipe (`applyBasinRecipe`).
    Apply,
    /// The cycle ended: look at the basin again (`basinChecker.scheduleUpdate`).
    Done,
}

/// `Mth.log2`: the floor of the base-2 logarithm.
fn log2(v: i32) -> i32 {
    31 - (v.max(1) as u32).leading_zeros() as i32
}

impl Mixer {
    /// `startProcessingBasin`.
    pub fn start(&mut self, recipe: usize) {
        if self.running && self.running_ticks <= 20 {
            return;
        }
        self.running = true;
        self.running_ticks = 0;
        self.processing_ticks = -1;
        self.recipe = Some(recipe);
    }

    /// One server tick at `speed` (rpm, signed); `time` the recipe's processing duration.
    pub fn tick(&mut self, speed: f32, time: i32) -> MixStep {
        if self.running_ticks >= 40 {
            self.running = false;
            self.running_ticks = 0;
            return MixStep::Done;
        }
        if !self.running {
            return MixStep::None;
        }
        let speed = speed.abs();
        let mut step = MixStep::None;
        if speed == 0.0 {
            if self.running_ticks < 20 {
                self.running_ticks = 40 - self.running_ticks;
            } else if self.running_ticks == 20 {
                self.running_ticks += 1;
            }
        }
        if self.running_ticks == 20 {
            if self.processing_ticks < 0 {
                let recipe_speed = if time != 0 { time as f32 / 100.0 } else { 1.0 };
                self.processing_ticks =
                    (log2((512.0 / speed) as i32) * (recipe_speed * 15.0).ceil() as i32 + 1).max(1);
            } else {
                self.processing_ticks -= 1;
                if self.processing_ticks == 0 {
                    self.running_ticks += 1;
                    self.processing_ticks = -1;
                    step = MixStep::Apply;
                }
            }
        }
        if self.running_ticks != 20 {
            self.running_ticks += 1;
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alloy() -> Recipe {
        Recipe {
            id: "create:mixing/andesite_alloy".into(),
            ingredients: vec![
                vec!["minecraft:andesite".into()],
                vec!["minecraft:iron_nugget".into(), "create:zinc_nugget".into()],
            ],
            results: vec![Output {
                item: "create:andesite_alloy".into(),
                count: 1,
                chance: 1.0,
            }],
            time: 0,
            heat: HeatReq::None,
            fluids: vec![],
            fluid_results: vec![],
        }
    }

    #[test]
    fn a_recipe_takes_one_of_each_and_fills_the_outputs() {
        let mut slots: Vec<Slot> = vec![None; SLOTS];
        slots[0] = Some(("minecraft:andesite".into(), 2));
        slots[1] = Some(("minecraft:iron_nugget".into(), 1));
        let r = alloy();
        assert_eq!(
            matching(
                std::slice::from_ref(&r),
                &slots,
                &Tanks::default(),
                Heat::None,
                |_| 64
            ),
            vec![0]
        );
        assert!(apply(
            &mut slots,
            &mut Tanks::default(),
            &r,
            Heat::None,
            |_| 64,
            &mut Rng::new(1),
            false
        ));
        assert_eq!(slots[0], Some(("minecraft:andesite".into(), 1)));
        assert_eq!(slots[1], None);
        assert_eq!(slots[9], Some(("create:andesite_alloy".into(), 1)));
        assert!(
            !apply(
                &mut slots,
                &mut Tanks::default(),
                &r,
                Heat::None,
                |_| 64,
                &mut Rng::new(1),
                true
            ),
            "no nugget left"
        );
        let brass = Recipe {
            heat: HeatReq::Heated,
            ..alloy()
        };
        slots[1] = Some(("minecraft:iron_nugget".into(), 1));
        assert!(!apply(
            &mut slots,
            &mut Tanks::default(),
            &brass,
            Heat::Smouldering,
            |_| 64,
            &mut Rng::new(1),
            true
        ));
        assert!(apply(
            &mut slots,
            &mut Tanks::default(),
            &brass,
            Heat::Kindled,
            |_| 64,
            &mut Rng::new(1),
            true
        ));
    }

    #[test]
    fn chocolate_takes_milk_from_a_tank_and_fills_an_output() {
        // data/create/recipe/mixing/chocolate.json: sugar, cocoa beans, 250 milk -> 250 chocolate.
        let r = Recipe {
            id: "create:mixing/chocolate".into(),
            ingredients: vec![
                vec!["minecraft:sugar".into()],
                vec!["minecraft:cocoa_beans".into()],
            ],
            results: vec![],
            time: 0,
            heat: HeatReq::Heated,
            fluids: vec![(Fluid::Milk, 250)],
            fluid_results: vec![(Fluid::Chocolate, 250)],
        };
        let mut slots: Vec<Slot> = vec![None; SLOTS];
        slots[0] = Some(("minecraft:sugar".into(), 2));
        slots[1] = Some(("minecraft:cocoa_beans".into(), 2));
        let mut tanks = Tanks::default();
        assert!(
            !apply(
                &mut slots.clone(),
                &mut tanks.clone(),
                &r,
                Heat::Kindled,
                |_| 64,
                &mut Rng::new(1),
                true
            ),
            "no milk"
        );
        assert_eq!(fill(&mut tanks.input, Fluid::Milk, 400), 400);
        assert_eq!(
            matching(
                std::slice::from_ref(&r),
                &slots,
                &tanks,
                Heat::Kindled,
                |_| 64
            ),
            vec![0]
        );
        assert!(apply(
            &mut slots,
            &mut tanks,
            &r,
            Heat::Kindled,
            |_| 64,
            &mut Rng::new(1),
            false
        ));
        assert_eq!(tanks.input[0], Some((Fluid::Milk, 150)));
        assert_eq!(tanks.output[0], Some((Fluid::Chocolate, 250)));
        assert!(
            !apply(
                &mut slots.clone(),
                &mut tanks.clone(),
                &r,
                Heat::Kindled,
                |_| 64,
                &mut Rng::new(1),
                true
            ),
            "150 left"
        );
        assert_eq!(
            drain(&mut tanks.output, 1000),
            Some((Fluid::Chocolate, 250))
        );
    }

    #[test]
    fn the_mixer_goes_down_mixes_and_comes_back() {
        let mut m = Mixer::default();
        m.start(0);
        let mut applied_at = None;
        for t in 0..200 {
            match m.tick(64.0, 0) {
                MixStep::Apply => applied_at = Some(t),
                MixStep::Done => {
                    // 64 rpm: log2(8) × 15 + 1 = 46 ticks at the bottom.
                    assert_eq!(applied_at, Some(20 + 46));
                    // The applying tick moves on twice (21, then 22): 19 more ticks up.
                    assert_eq!(t, applied_at.unwrap() + 19);
                    return;
                }
                MixStep::None => {}
            }
        }
        panic!("the cycle never ended");
    }
}
