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

/// `PotionFluidHandler.getRequiredAmountForFilledBottle`: a glass bottle takes a quarter bucket.
pub const BOTTLE: u32 = 250;

/// What a glass bottle filled with water becomes (`PotionFluidHandler.fillBottle`): a potion,
/// its contents water (named as `tank::potion_item` does; the glue gives the stack its
/// `potion_contents`).
pub const WATER_BOTTLE: &str = "minecraft:potion|minecraft:water";

/// `GenericItemFilling.getRequiredAmountForItem` and `fillItem`: a filling recipe for the item
/// and fluid; a glass bottle filled with water, builder's tea or a potion
/// (`canFillGlassBottleInternally`); else an empty bucket taking a bucket's worth of a fluid
/// with a bucket. How much it takes and what it becomes.
pub fn filling(item: &str, fluid: Fluid, recipes: &[Filling]) -> Option<(u32, String)> {
    if let Some(r) = recipes.iter().find(|r| r.item == item && r.fluid == fluid) {
        return Some((r.amount, r.result.clone()));
    }
    if item == "minecraft:glass_bottle" {
        return match fluid {
            Fluid::Water => Some((BOTTLE, WATER_BOTTLE.to_owned())),
            Fluid::Tea => Some((BOTTLE, "create:builders_tea".to_owned())),
            Fluid::Potion(p) => Some((BOTTLE, p.item())),
            _ => None,
        };
    }
    if item != "minecraft:bucket" {
        return None;
    }
    fluid.bucket().map(|b| (BUCKET, b.to_owned()))
}

/// SpoutBlockEntity.tick advances its timer on the spout's own registration turn.
/// Item callbacks run separately on the depot or belt controller's turn.
pub fn tick(s: &mut Spout) {
    if s.processing >= 0 {
        s.processing -= 1;
    }
}

/// `FillingBySpout.canItemBeFilled`: a filling recipe takes the item (with any fluid), or it is
/// an empty bucket or a glass bottle.
pub fn fillable(item: &str, recipes: &[Filling]) -> bool {
    matches!(item, "minecraft:bucket" | "minecraft:glass_bottle")
        || recipes.iter().any(|r| r.item == item)
}

/// What a spout does to a stack on the belt below.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnBelt {
    Pass,
    Hold,
    /// One of the stack is filled: what it became.
    Fill(String),
}

/// The spout's belt callbacks, `onItemReceived` (not `locked`) and `whenItemHeld`: it holds a
/// stack it can fill, starts its cycle when it has enough fluid, and fills one at the cycle's
/// 5th-last tick (the cycle runs down in `tick`).
pub fn on_belt(s: &mut Spout, item: &str, recipes: &[Filling], locked: bool) -> OnBelt {
    if locked && s.processing != -1 && s.processing != 5 {
        return OnBelt::Hold;
    }
    if !fillable(item, recipes) {
        return OnBelt::Pass;
    }
    let Some((fluid, have)) = s.fluid else {
        return OnBelt::Hold;
    };
    let Some((need, result)) = filling(item, fluid, recipes) else {
        return OnBelt::Pass;
    };
    if !locked || need > have {
        return OnBelt::Hold;
    }
    if s.processing == -1 {
        s.processing = FILLING_TIME;
        return OnBelt::Hold;
    }
    s.fluid = (have > need).then_some((fluid, have - need));
    OnBelt::Fill(result)
}

/// DepotBehaviour.tick dispatches on the previous lock, then applies HOLD to the
/// transported stack, including the replacement produced by convertToAndLeaveHeld.
pub fn on_depot(
    s: &mut Spout,
    held: &mut crate::depot::Held,
    item: &str,
    recipes: &[Filling],
) -> OnBelt {
    let result = on_belt(s, item, recipes, held.locked);
    held.locked = result != OnBelt::Pass;
    result
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

    fn step(s: &mut Spout, item: Option<&str>, recipes: &[Filling]) -> Option<String> {
        tick(s);
        match on_belt(s, item?, recipes, true) {
            OnBelt::Fill(out) => Some(out),
            _ => None,
        }
    }

    #[test]
    fn depot_receives_before_starting_and_keeps_the_filled_item_locked() {
        let mut s = Spout::default();
        fill(&mut s, Fluid::Water, 1000);
        let mut held = crate::depot::Held::new(0, false);
        assert_eq!(
            on_depot(&mut s, &mut held, "minecraft:bucket", &[]),
            OnBelt::Hold
        );
        assert!(held.locked);
        assert_eq!(s.processing, -1);
        assert_eq!(
            on_depot(&mut s, &mut held, "minecraft:bucket", &[]),
            OnBelt::Hold
        );
        assert_eq!(s.processing, 20);
        for _ in 0..15 {
            step(&mut s, None, &[]);
        }
        assert_eq!(
            on_depot(&mut s, &mut held, "minecraft:bucket", &[]),
            OnBelt::Fill("minecraft:water_bucket".into())
        );
        assert!(held.locked);
        step(&mut s, None, &[]);
        assert_eq!(
            on_depot(&mut s, &mut held, "minecraft:water_bucket", &[]),
            OnBelt::Hold
        );
        assert!(held.locked);
    }

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
    fn a_glass_bottle_takes_a_quarter_bucket_of_water() {
        let mut s = Spout::default();
        fill(&mut s, Fluid::Water, 1000);
        let made = (0..200)
            .filter_map(|_| step(&mut s, Some("minecraft:glass_bottle"), &[]))
            .count();
        assert_eq!(made, 4);
        assert!(filling("minecraft:glass_bottle", Fluid::Lava, &[]).is_none());
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

    #[test]
    fn a_belt_stack_is_held_and_filled_after_the_cycle() {
        let mut s = Spout::default();
        assert_eq!(on_belt(&mut s, "minecraft:stone", &[], false), OnBelt::Pass);
        assert_eq!(
            on_belt(&mut s, "minecraft:bucket", &[], false),
            OnBelt::Hold,
            "held while the spout is empty"
        );
        fill(&mut s, Fluid::Water, 1000);
        assert_eq!(on_belt(&mut s, "minecraft:bucket", &[], true), OnBelt::Hold);
        assert_eq!(s.processing, FILLING_TIME, "the cycle starts");
        let mut ticks = 0;
        let filled = loop {
            // The spout's tick runs the cycle down; the belt asks after it.
            step(&mut s, None, &[]);
            ticks += 1;
            if let OnBelt::Fill(r) = on_belt(&mut s, "minecraft:bucket", &[], true) {
                break r;
            }
        };
        assert_eq!((ticks, filled.as_str()), (15, "minecraft:water_bucket"));
        assert_eq!(s.fluid, None);
    }
}
