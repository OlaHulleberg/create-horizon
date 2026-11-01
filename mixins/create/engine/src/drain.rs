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

/// `GenericItemEmptying` for what is ported: a filled bucket gives a bucket's worth.
pub fn contents(item: &str) -> Option<Fluid> {
    Fluid::of_bucket(item)
}

/// A tick (`tick`, `continueProcessing`): true when one item of the held stack was emptied now
/// (the caller throws its empty bucket off).
pub fn tick(d: &mut Drain) -> bool {
    let Some((item, count)) = d.held.clone() else {
        d.processing = 0;
        return false;
    };
    let Some(fluid) = contents(&item) else {
        return false;
    };
    if d.processing == 0 {
        d.processing = FILLING_TIME;
        return false;
    }
    d.processing -= 1;
    let room = d
        .fluid
        .is_none_or(|(f, n)| f == fluid && n + BUCKET <= CAPACITY);
    if d.processing > 5 {
        if !room {
            d.processing = FILLING_TIME;
        }
        return false;
    }
    if d.processing != 5 || !room {
        return false;
    }
    d.fluid = Some((fluid, d.fluid.map_or(0, |f| f.1) + BUCKET));
    d.held = (count > 1).then(|| (item, count - 1));
    d.processing = 0;
    true
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
        let emptied = (0..100).filter(|_| tick(&mut d)).count();
        assert_eq!(emptied, 1, "1,500 mB: room for one bucket");
        assert_eq!(d.fluid, Some((Fluid::Water, 1000)));
        assert_eq!(drain(&mut d, 600), Some((Fluid::Water, 600)));
        assert!((0..40).any(|_| tick(&mut d)), "room again for the second");
        assert_eq!(d.held, None);
    }
}
