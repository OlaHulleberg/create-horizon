//! Create's item drain (`ItemDrainBlockEntity`, `GenericItemEmptying`): items on it are emptied
//! into its 1,500 mB tank, one per 20-tick cycle, when the tank has room.

use crate::spout::FILLING_TIME;
use crate::tank::{BUCKET, Fluid};

/// `SmartFluidTankBehaviour.single(this, 1500)`.
pub const CAPACITY: u32 = 1500;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drain {
    /// The held stack (`heldItem`).
    pub held: Option<(String, u8)>,
    pub processing: i32,
    pub fluid: Option<(Fluid, u32)>,
}

/// What emptying an item gives (`GenericItemEmptying`): a fluid and how much, and what is left
/// of the item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emptying {
    pub fluid: Fluid,
    pub amount: u32,
    pub leftover: Option<String>,
}

/// `GenericItemEmptying.emptyItem`: a `create:emptying` recipe for the item (`recipes`, by
/// item), else a filled bucket's worth leaving the bucket.
pub fn contents(item: &str, recipes: &[(String, Emptying)]) -> Option<Emptying> {
    if let Some((_, e)) = recipes.iter().find(|(i, _)| i == item) {
        return Some(e.clone());
    }
    Fluid::of_bucket(item).map(|fluid| Emptying {
        fluid,
        amount: BUCKET,
        leftover: Some("minecraft:bucket".into()),
    })
}

/// A tick (`tick`, `continueProcessing`): when one item of the held stack was emptied now, what
/// is left of it (the caller throws it off: an empty bucket, a glass bottle), or None.
pub fn tick(d: &mut Drain, recipes: &[(String, Emptying)]) -> Option<Option<String>> {
    let Some((item, count)) = d.held.clone() else {
        d.processing = 0;
        return None;
    };
    let Emptying {
        fluid,
        amount,
        leftover,
    } = contents(&item, recipes)?;
    if d.processing == 0 {
        d.processing = FILLING_TIME;
        return None;
    }
    d.processing -= 1;
    let room = d
        .fluid
        .is_none_or(|(f, n)| f == fluid && n + amount <= CAPACITY);
    if d.processing > 5 {
        if !room {
            d.processing = FILLING_TIME;
        }
        return None;
    }
    if d.processing != 5 || !room {
        return None;
    }
    d.fluid = Some((fluid, d.fluid.map_or(0, |f| f.1) + amount));
    d.held = (count > 1).then(|| (item, count - 1));
    d.processing = 0;
    Some(leftover)
}

/// Drained by a pump: up to `amount`.
pub fn drain(d: &mut Drain, amount: u32) -> Option<(Fluid, u32)> {
    let (fluid, have) = d.fluid?;
    let out = amount.min(have);
    d.fluid = (have > out).then_some((fluid, have - out));
    (out > 0).then_some((fluid, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_empty_one_cycle_each_until_full() {
        let mut d = Drain {
            held: Some(("minecraft:water_bucket".into(), 2)),
            ..Drain::default()
        };
        let emptied = (0..100).filter(|_| tick(&mut d, &[]).is_some()).count();
        assert_eq!(emptied, 1, "1,500 mB: room for one bucket");
        assert_eq!(d.fluid, Some((Fluid::Water, 1000)));
        assert_eq!(drain(&mut d, 600), Some((Fluid::Water, 600)));
        let left = (0..40).find_map(|_| tick(&mut d, &[]));
        assert_eq!(
            left,
            Some(Some("minecraft:bucket".into())),
            "room again for the second"
        );
        assert_eq!(d.held, None);
    }

    #[test]
    fn honey_bottles_empty_by_their_recipe() {
        // data/create/recipe/emptying/honey_bottle.json
        let recipes = vec![(
            "minecraft:honey_bottle".to_owned(),
            Emptying {
                fluid: Fluid::Honey,
                amount: 250,
                leftover: Some("minecraft:glass_bottle".into()),
            },
        )];
        let mut d = Drain {
            held: Some(("minecraft:honey_bottle".into(), 7)),
            ..Drain::default()
        };
        let bottles = (0..400).filter_map(|_| tick(&mut d, &recipes)).count();
        assert_eq!(bottles, 6, "1,500 mB holds six bottles");
        assert_eq!(d.fluid, Some((Fluid::Honey, 1500)));
        assert_eq!(d.held, Some(("minecraft:honey_bottle".into(), 1)));
    }
}
