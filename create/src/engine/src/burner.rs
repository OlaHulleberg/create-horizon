//! Create's blaze burner (`BlazeBurnerBlockEntity`, `BlazeBurnerBlock`): fuel given to the blaze
//! burns down tick by tick and sets its heat (smouldering without fuel, kindled, fading near the
//! end, seething on special fuel). A burner with nothing burning does nothing.

use crate::fan::Heat;

pub const MAX_HEAT_CAPACITY: i32 = 10_000;
pub const INSERTION_THRESHOLD: i32 = 500;

/// `BlazeBurnerBlockEntity.FuelType`, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fuel {
    #[default]
    None,
    Normal,
    Special,
}

impl Fuel {
    pub fn index(self) -> i32 {
        self as i32
    }
    pub fn from_index(i: i32) -> Fuel {
        match i {
            1 => Fuel::Normal,
            2 => Fuel::Special,
            _ => Fuel::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Burner {
    pub fuel: Fuel,
    pub remaining: i32,
    /// `isCreative`: fed a creative blaze cake, it holds this heat and takes no fuel.
    pub creative: Option<Heat>,
}

impl Burner {
    /// Whether it has anything to burn down (the only burners that tick).
    pub fn burning(&self) -> bool {
        self.creative.is_none() && (self.fuel != Fuel::None || self.remaining > 0)
    }

    /// Whether it has anything to save: fuel, or a creative heat.
    pub fn saved(&self) -> bool {
        self.creative.is_some() || self.burning()
    }

    /// `applyCreativeFuel`: no fuel, creative, the heat after `shown`, the block's
    /// (`getHeatLevelFromBlock().nextActiveLevel()`, fading skipped): smouldering, kindled,
    /// seething, round.
    pub fn creative_fuel(&mut self, shown: Heat) -> Heat {
        let next = match shown {
            Heat::Smouldering | Heat::Fading => Heat::Kindled,
            Heat::Kindled => Heat::Seething,
            Heat::None | Heat::Seething => Heat::Smouldering,
        };
        *self = Burner {
            creative: Some(next),
            ..Burner::default()
        };
        next
    }

    /// `getHeatLevel`.
    pub fn heat(&self) -> Heat {
        if let Some(h) = self.creative {
            return h;
        }
        match self.fuel {
            Fuel::Special => Heat::Seething,
            Fuel::Normal if (self.remaining as f64) / (MAX_HEAT_CAPACITY as f64) < 0.0125 => {
                Heat::Fading
            }
            Fuel::Normal => Heat::Kindled,
            Fuel::None => Heat::Smouldering,
        }
    }

    /// `tryUpdateFuel` for an item that is `fuel` burning `time` ticks; `overflow` lets a player
    /// top normal fuel up to the capacity. False when it doesn't take it.
    pub fn try_fuel(&mut self, fuel: Fuel, time: i32, overflow: bool) -> bool {
        if self.creative.is_some() || fuel == Fuel::None || fuel < self.fuel {
            return false;
        }
        let mut time = time;
        if fuel == self.fuel {
            if self.remaining <= INSERTION_THRESHOLD {
                time += self.remaining;
            } else if overflow && fuel == Fuel::Normal {
                time = if self.remaining < MAX_HEAT_CAPACITY {
                    (self.remaining + time).min(MAX_HEAT_CAPACITY)
                } else {
                    self.remaining
                };
            } else {
                return false;
            }
        }
        self.fuel = fuel;
        self.remaining = time;
        true
    }

    /// `tick` on the server (not creative): the time burns down; special fuel ends in half a
    /// capacity of normal fuel.
    pub fn tick(&mut self) {
        if self.remaining > 0 {
            self.remaining -= 1;
        }
        if self.remaining > 0 {
            return;
        }
        if self.fuel == Fuel::Special {
            self.fuel = Fuel::Normal;
            self.remaining = MAX_HEAT_CAPACITY / 2;
        } else {
            self.fuel = Fuel::None;
        }
    }
}

/// The `blaze` property's value for a heat level.
pub fn heat_name(h: Heat) -> &'static str {
    match h {
        Heat::None => "none",
        Heat::Smouldering => "smouldering",
        Heat::Fading => "fading",
        Heat::Kindled => "kindled",
        Heat::Seething => "seething",
    }
}

/// `BlazeBurnerRenderer.getBlazeModel` with nothing above it: the head's partial.
pub fn head(h: Heat) -> &'static str {
    match h {
        Heat::Seething => "part_blaze_super",
        Heat::Fading | Heat::Kindled => "part_blaze_idle",
        _ => "part_blaze_inert",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coal_burns_down_through_the_heat_levels() {
        let mut b = Burner::default();
        assert_eq!(b.heat(), Heat::Smouldering);
        assert!(b.try_fuel(Fuel::Normal, 1600, true));
        assert_eq!(b.heat(), Heat::Kindled);
        // A second coal tops it up (a player overflows past the threshold).
        assert!(b.try_fuel(Fuel::Normal, 1600, true));
        assert_eq!(b.remaining, 3200);
        assert!(
            !b.try_fuel(Fuel::Normal, 1600, false),
            "a machine can't overfill it"
        );
        for _ in 0..3200 - 124 {
            b.tick();
        }
        assert_eq!(b.heat(), Heat::Fading, "under 1.25% of capacity");
        for _ in 0..124 {
            b.tick();
        }
        assert_eq!((b.fuel, b.heat()), (Fuel::None, Heat::Smouldering));
        assert!(!b.burning());
    }

    #[test]
    fn a_blaze_cake_superheats_then_falls_back() {
        let mut b = Burner::default();
        assert!(b.try_fuel(Fuel::Special, 3200, true));
        assert!(
            !b.try_fuel(Fuel::Normal, 1600, true),
            "normal fuel can't replace special"
        );
        assert_eq!(b.heat(), Heat::Seething);
        for _ in 0..3200 {
            b.tick();
        }
        assert_eq!((b.fuel, b.remaining), (Fuel::Normal, 5000));
        assert_eq!(head(b.heat()), "part_blaze_idle");
    }

    #[test]
    fn a_creative_cake_cycles_the_heat_and_refuses_fuel() {
        let mut b = Burner::default();
        assert!(b.try_fuel(Fuel::Normal, 1600, true));
        assert_eq!(
            b.creative_fuel(b.heat()),
            Heat::Seething,
            "kindled goes on to seething"
        );
        assert_eq!(b.creative_fuel(b.heat()), Heat::Smouldering);
        assert_eq!(
            b.creative_fuel(b.heat()),
            Heat::Kindled,
            "fading is skipped"
        );
        // The block's heat counts, not what the burner last burned.
        assert_eq!(b.creative_fuel(Heat::Smouldering), Heat::Kindled);
        assert!(!b.try_fuel(Fuel::Special, 3200, true));
        assert!(!b.burning() && b.saved());
        assert_eq!(b.heat(), Heat::Kindled);
    }
}
