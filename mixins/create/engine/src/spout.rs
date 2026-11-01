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

/// `GenericItemFilling` for what is ported: an empty bucket takes a bucket's worth.
pub fn fill_amount(item: &str) -> Option<u32> {
    (item == "minecraft:bucket").then_some(BUCKET)
}

/// What the item becomes filled with `fluid`.
pub fn filled(item: &str, fluid: Fluid) -> Option<&'static str> {
    (item == "minecraft:bucket").then(|| fluid.bucket())
}

/// `whenItemHeld` and `tick` for a held `item`: true when it is filled now.
pub fn step(s: &mut Spout, item: Option<&str>) -> bool {
    if s.processing >= 0 {
        s.processing -= 1;
    }
    let Some(item) = item else {
        return false;
    };
    if s.processing != -1 && s.processing != 5 {
        return false;
    }
    let (Some(need), Some((_, have))) = (fill_amount(item), s.fluid) else {
        return false;
    };
    if need > have {
        return false;
    }
    if s.processing == -1 {
        s.processing = FILLING_TIME;
        return false;
    }
    let (fluid, have) = s.fluid.unwrap();
    s.fluid = (have > need).then_some((fluid, have - need));
    true
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
        assert!(!step(&mut s, Some("minecraft:bucket")), "the cycle starts");
        let mut ticks = 1;
        while !step(&mut s, Some("minecraft:bucket")) {
            ticks += 1;
        }
        assert_eq!(ticks, 15, "filled when 5 of the 20 ticks are left");
        assert_eq!(s.fluid, None);
        assert_eq!(
            filled("minecraft:bucket", Fluid::Water),
            Some("minecraft:water_bucket")
        );
        assert!(!step(&mut s, Some("minecraft:stone")));
    }
}
