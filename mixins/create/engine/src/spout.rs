//! Create's spout (`SpoutBlockEntity`, `FillingBySpout`, `GenericItemFilling`): a one-bucket
//! tank that fills the container held on the depot or belt two below, one per 20-tick cycle.

use crate::tank::{BUCKET, Fluid};

/// `SmartFluidTankBehaviour.single(this, 1000)`.
pub const CAPACITY: u32 = BUCKET;
/// `FILLING_TIME`.
pub const FILLING_TIME: i32 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spout {
    pub fluid: Option<(Fluid, u32)>,
    pub processing: i32,
}

impl Default for Spout {
    fn default() -> Spout {
        Spout {
            fluid: None,
            processing: -1,
        }
    }
}

/// A `create:filling` recipe: the item, the fluid and how much of it, what it becomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filling {
    pub item: String,
    pub fluid: Fluid,
    pub amount: u32,
    pub result: String,
}

/// `GenericItemFilling.getRequiredAmountForItem` and `fillItem`: a filling recipe for the item
/// and fluid, else an empty bucket taking a bucket's worth. How much it takes and what it
/// becomes.
pub fn filling(item: &str, fluid: Fluid, recipes: &[Filling]) -> Option<(u32, String)> {
    if let Some(r) = recipes.iter().find(|r| r.item == item && r.fluid == fluid) {
        return Some((r.amount, r.result.clone()));
    }
    (item == "minecraft:bucket").then(|| (BUCKET, fluid.bucket().to_owned()))
}

/// `whenItemHeld` and `tick` for a held `item`: what it became when it is filled now.
pub fn step(s: &mut Spout, item: Option<&str>, recipes: &[Filling]) -> Option<String> {
    if s.processing >= 0 {
        s.processing -= 1;
    }
    let item = item?;
    if s.processing != -1 && s.processing != 5 {
        return None;
    }
    let (fluid, have) = s.fluid?;
    let (need, result) = filling(item, fluid, recipes)?;
    if need > have {
        return None;
    }
    if s.processing == -1 {
        s.processing = FILLING_TIME;
        return None;
    }
    s.fluid = (have > need).then_some((fluid, have - need));
    Some(result)
}

/// Fluid put into the spout's tank: what it took.
pub fn fill(s: &mut Spout, fluid: Fluid, amount: u32) -> u32 {
    if s.fluid.is_some_and(|f| f.0 != fluid) {
        return 0;
    }
    let have = s.fluid.map_or(0, |f| f.1);
    let took = amount.min(CAPACITY - have);
    if took > 0 {
        s.fluid = Some((fluid, have + took));
    }
    took
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bucket_is_filled_after_the_cycle() {
        let mut s = Spout::default();
        assert_eq!(fill(&mut s, Fluid::Water, 1500), 1000);
        assert!(
            step(&mut s, Some("minecraft:bucket"), &[]).is_none(),
            "the cycle starts"
        );
        let mut ticks = 1;
        let filled = loop {
            if let Some(r) = step(&mut s, Some("minecraft:bucket"), &[]) {
                break r;
            }
            ticks += 1;
        };
        assert_eq!(ticks, 15, "filled when 5 of the 20 ticks are left");
        assert_eq!(filled, "minecraft:water_bucket");
        assert_eq!(s.fluid, None);
        assert!(step(&mut s, Some("minecraft:stone"), &[]).is_none());
    }

    #[test]
    fn an_apple_takes_honey_by_its_recipe() {
        // data/create/recipe/filling/honeyed_apple.json
        let recipes = vec![Filling {
            item: "minecraft:apple".into(),
            fluid: Fluid::Honey,
            amount: 250,
            result: "create:honeyed_apple".into(),
        }];
        let mut s = Spout::default();
        fill(&mut s, Fluid::Honey, 1000);
        let made = (0..200)
            .filter_map(|_| step(&mut s, Some("minecraft:apple"), &recipes))
            .count();
        assert_eq!(made, 4, "a bucket of honey glazes four apples");
        assert!(filling("minecraft:apple", Fluid::Water, &recipes).is_none());
    }
}
