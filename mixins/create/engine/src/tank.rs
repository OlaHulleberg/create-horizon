//! Create's fluid tank (`FluidTankBlock`, `FluidTankBlockEntity`, `FluidTankRenderer`), one block
//! for now: 8 buckets (`fluidTankCapacity`) of one fluid, buckets in and out for creative players
//! (Create 6 leaves survival to spouts and drains), drawn as a box of the fluid at its level.

/// The fluids a tank can hold without fluids of its own yet (gap 5): vanilla's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fluid {
    Water,
    Lava,
}

impl Fluid {
    pub fn index(self) -> u8 {
        self as u8
    }
    pub fn from_index(i: u8) -> Option<Fluid> {
        match i {
            0 => Some(Fluid::Water),
            1 => Some(Fluid::Lava),
            _ => None,
        }
    }
    /// The bucket that holds it.
    pub fn bucket(self) -> &'static str {
        match self {
            Fluid::Water => "minecraft:water_bucket",
            Fluid::Lava => "minecraft:lava_bucket",
        }
    }
    pub fn of_bucket(item: &str) -> Option<Fluid> {
        match item {
            "minecraft:water_bucket" => Some(Fluid::Water),
            "minecraft:lava_bucket" => Some(Fluid::Lava),
            _ => None,
        }
    }
}

/// `FluidType.BUCKET_VOLUME`.
pub const BUCKET: u32 = 1000;
/// `getCapacityMultiplier`: 8 buckets a block (`fluidTankCapacity`).
pub const CAPACITY: u32 = 8 * BUCKET;

/// A tank's contents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tank {
    pub fluid: Option<Fluid>,
    pub amount: u32,
}

impl Tank {
    /// `FluidHelper.tryEmptyItemIntoBE` for a bucket: all of it or nothing.
    pub fn fill(&mut self, fluid: Fluid) -> bool {
        if self.fluid.is_some_and(|f| f != fluid) || self.amount + BUCKET > CAPACITY {
            return false;
        }
        self.fluid = Some(fluid);
        self.amount += BUCKET;
        true
    }

    /// `FluidHelper.tryFillItemFromBE` for an empty bucket: a whole bucket's worth or nothing.
    pub fn drain(&mut self) -> Option<Fluid> {
        let fluid = self.fluid?;
        if self.amount < BUCKET {
            return None;
        }
        self.amount -= BUCKET;
        if self.amount == 0 {
            self.fluid = None;
        }
        Some(fluid)
    }
}

/// `FluidTankRenderer.renderSafe` for a one-block tank: the fluid's box, from its floor (cap and
/// puddle above the block's bottom) up to its level, inside the hull; None when it is too little
/// to show.
pub fn fluid_box(t: &Tank) -> Option<([f32; 3], [f32; 3])> {
    t.fluid?;
    let (height, width) = (1.0f32, 1.0f32);
    let cap = 1.0 / 4.0;
    let hull = 1.0 / 16.0 + 1.0 / 128.0;
    let puddle = 1.0 / 16.0;
    let total = height - 2.0 * cap - puddle;
    let level = t.amount as f32 / CAPACITY as f32;
    if level < 1.0 / (512.0 * total) {
        return None;
    }
    let clamped = (level * total).clamp(0.0, total);
    // yMin = total + cap + puddle - clamped, then translated down by total - clamped.
    let y0 = cap + puddle;
    Some(([hull, y0, hull], [width - hull, y0 + clamped, width - hull]))
}

/// Tanks' contents as clients draw them: position, fluid (255: none), amount; 17 bytes each.
pub const CHANNEL: &str = "create:tank";

pub fn encode(all: &[([i32; 3], Tank)]) -> Vec<Vec<u8>> {
    all.chunks(30_000 / 17)
        .map(|chunk| {
            let mut out = Vec::new();
            for (p, t) in chunk {
                for v in p {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                out.push(t.fluid.map_or(255, Fluid::index));
                out.extend_from_slice(&t.amount.to_le_bytes());
            }
            out
        })
        .collect()
}

pub fn decode(data: &[u8]) -> Vec<([i32; 3], Tank)> {
    let i = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    data.chunks_exact(17)
        .map(|r| {
            (
                [i(&r[0..4]), i(&r[4..8]), i(&r[8..12])],
                Tank {
                    fluid: Fluid::from_index(r[12]),
                    amount: u32::from_le_bytes(r[13..17].try_into().unwrap()),
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_go_in_and_out_whole() {
        let mut t = Tank::default();
        for _ in 0..8 {
            assert!(t.fill(Fluid::Water));
        }
        assert!(!t.fill(Fluid::Water), "eight buckets fill it");
        assert!(
            !Tank {
                fluid: Some(Fluid::Water),
                amount: 1000
            }
            .clone()
            .fill(Fluid::Lava)
        );
        assert_eq!(t.drain(), Some(Fluid::Water));
        assert_eq!(t.amount, 7000);
        let mut one = Tank {
            fluid: Some(Fluid::Lava),
            amount: BUCKET,
        };
        assert_eq!(one.drain(), Some(Fluid::Lava));
        assert_eq!(one, Tank::default());
        assert_eq!(one.drain(), None);
    }

    #[test]
    fn the_fluid_rises_from_the_tank_floor() {
        let half = Tank {
            fluid: Some(Fluid::Water),
            amount: CAPACITY / 2,
        };
        let (min, max) = fluid_box(&half).unwrap();
        assert_eq!(min[1], 0.3125);
        // Half of the 7/16 between the caps and the puddle.
        assert!((max[1] - (0.3125 + 0.4375 / 2.0)).abs() < 1e-6);
        assert!((min[0] - (1.0 / 16.0 + 1.0 / 128.0)).abs() < 1e-6);
        assert_eq!(fluid_box(&Tank::default()), None);
        let all = vec![([1, 2, 3], half)];
        assert_eq!(decode(&encode(&all)[0]), all);
    }
}
