//! Create's diving gear, backtank and extendo grip (`content/equipment/armor`,
//! `content/equipment/extendoGrip`): the backtank filled by its shaft (`BacktankBlockEntity.tick`),
//! the air kept on the item (`BacktankUtil`), the diving helmet's breathing
//! (`DivingHelmetItem.breatheUnderwater`), the diving boots' sinking (`DivingBootsItem`) and the
//! extendo grip's reach (`ExtendoGripItem`), with `CEquipment`'s default numbers.

/// `CEquipment.airInBacktank`: air in a backtank without Capacity (seconds of breathing).
pub const AIR_IN_BACKTANK: i32 = 900;
/// `CEquipment.enchantedBacktankCapacity`: air added by each level of Capacity.
pub const ENCHANTED_BACKTANK_CAPACITY: i32 = 300;
/// `CEquipment.maxExtendoGripActions`: free extendo grip actions in one full copper backtank.
pub const MAX_EXTENDO_GRIP_ACTIONS: i32 = 1000;
/// `Entity.getMaxAirSupply` of a player: what the helmet refills to.
pub const MAX_AIR_SUPPLY: i32 = 300;
/// `ExtendoGripItem.MAX_DAMAGE`.
pub const EXTENDO_GRIP_DURABILITY: u32 = 200;
/// The id of Create's backtank air (`AllDataComponents.BACKTANK_AIR`), kept in the stack's
/// `minecraft:custom_data` here (Horizon adds no component types).
pub const AIR_KEY: &str = "create:backtank_air";
/// `AllEnchantments.CAPACITY`.
pub const CAPACITY: &str = "create:capacity";

/// `BacktankUtil.maxAir(enchantLevel)`.
pub fn max_air(capacity_level: i32) -> i32 {
    AIR_IN_BACKTANK + ENCHANTED_BACKTANK_CAPACITY * capacity_level
}

/// `BacktankUtil.getAir`: the stored air, at most the stack's maximum.
pub fn air(stored: i32, capacity_level: i32) -> i32 {
    stored.min(max_air(capacity_level))
}

/// `ComparatorUtil.fractionToRedstoneLevel`.
pub fn fraction_to_redstone_level(frac: f64) -> i32 {
    (frac * 14.0 + if frac > 0.0 { 1.0 } else { 0.0 })
        .clamp(0.0, 15.0)
        .floor() as i32
}

/// Air added per fill of a backtank turning at `speed` RPM
/// (`Mth.clamp(((int) abs - 100) / 20, 1, 5)`).
pub fn fill_increment(speed: f32) -> i32 {
    ((speed.abs() as i32 - 100) / 20).clamp(1, 5)
}

/// Ticks waited after a fill (`Mth.clamp((int) (128f - abs / 5f) - 108, 0, 20)`).
pub fn fill_delay(speed: f32) -> i32 {
    ((128.0f32 - speed.abs() / 5.0) as i32 - 108).clamp(0, 20)
}

/// A placed backtank (`BacktankBlockEntity`): `airLevel`, `airLevelTimer` and
/// `capacityEnchantLevel`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Backtank {
    pub air: i32,
    pub timer: i32,
    pub capacity: i32,
}

/// What one fill changed: whether the comparator output moved, and whether it is now full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filled {
    pub comparator_changed: bool,
    pub full: bool,
}

impl Backtank {
    pub fn max_air(&self) -> i32 {
        max_air(self.capacity)
    }

    /// `getComparatorOutput`.
    pub fn comparator(&self) -> i32 {
        fraction_to_redstone_level(f64::from(self.air as f32 / self.max_air() as f32))
    }

    /// Whether its tick does anything: turning, not waterlogged, and waiting or not full.
    pub fn ticks(&self, speed: f32, waterlogged: bool) -> bool {
        speed != 0.0 && !waterlogged && (self.timer > 0 || self.air != self.max_air())
    }

    /// The server's `tick`: nothing without speed or when waterlogged; else the timer counts
    /// down, then air is added by the speed, up to the maximum, and the timer set again.
    pub fn tick(&mut self, speed: f32, waterlogged: bool) -> Option<Filled> {
        if speed == 0.0 || waterlogged {
            return None;
        }
        if self.timer > 0 {
            self.timer -= 1;
            return None;
        }
        let max = self.max_air();
        if self.air == max {
            return None;
        }
        let before = self.comparator();
        self.air = max.min(self.air + fill_increment(speed));
        let filled = Filled {
            comparator_changed: self.comparator() != before,
            full: self.air == max,
        };
        self.timer = fill_delay(speed);
        Some(filled)
    }
}

/// A warning `BacktankUtil.sendWarning` shows as a subtitle: the tank went to a tenth, or ran out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Warning {
    /// `create.backtank.low`, gold.
    Low,
    /// `create.backtank.depleted`, red.
    Depleted,
}

impl Warning {
    pub fn key(self) -> &'static str {
        match self {
            Warning::Low => "create.backtank.low",
            Warning::Depleted => "create.backtank.depleted",
        }
    }
}

/// `BacktankUtil.consumeAir`: the air left after taking `amount` from a tank holding `stored`
/// with `capacity_level`, and the warnings crossed (a tenth of the maximum, then 1).
pub fn consume_air(stored: i32, capacity_level: i32, amount: i32) -> (i32, Vec<Warning>) {
    let max = max_air(capacity_level);
    let before = air(stored, capacity_level);
    let after = (before - amount).max(0);
    let mut warnings = Vec::new();
    for threshold in [max as f32 / 10.0, 1.0] {
        // sendWarning: only when this use crossed the threshold.
        if after as f32 > threshold || before as f32 <= threshold {
            continue;
        }
        warnings.push(if threshold == 1.0 {
            Warning::Depleted
        } else {
            Warning::Low
        });
    }
    (after.min(max), warnings)
}

/// `BacktankUtil.canAbsorbDamage`'s cost for a tool with `uses_per_tank` free uses.
pub fn absorb_cost(uses_per_tank: i32) -> i32 {
    (AIR_IN_BACKTANK / uses_per_tank).max(1)
}

/// A worn tank as `BacktankUtil.getAllWithAir` sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tank {
    /// Its slot, for the glue.
    pub slot: usize,
    /// Its stored air and Capacity level.
    pub stored: i32,
    pub capacity: i32,
    /// `has(DataComponents.FIRE_RESISTANT)`: the netherite backtank.
    pub fire_resistant: bool,
}

impl Tank {
    pub fn air(&self) -> i32 {
        air(self.stored, self.capacity)
    }
}

/// `BacktankUtil.getAllWithAir`: the tanks with air left, the emptiest first (a stable sort, as
/// Java's `List.sort`).
pub fn with_air(tanks: &[Tank]) -> Vec<Tank> {
    let mut all: Vec<Tank> = tanks.iter().copied().filter(|t| t.air() > 0).collect();
    all.sort_by_key(|t| t.air());
    all
}

/// What the diving helmet does this tick (`DivingHelmetItem.breatheUnderwater`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Breath {
    /// The slot of the tank to take one air from (every 20 game ticks).
    pub consume: Option<usize>,
    /// Breathe (`setCanBreathe`) and refill the air supply to its maximum.
    pub refill: bool,
}

/// `DivingHelmetItem.breatheUnderwater` for a helmet (`Some(fire_resistant)`, the netherite one)
/// or none, in lava or not, able to breathe without it or not, with the worn tanks.
pub fn breathe(
    helmet: Option<bool>,
    in_lava: bool,
    can_breathe: bool,
    tanks: &[Tank],
    game_time: i64,
) -> Breath {
    let Some(fire_resistant) = helmet else {
        return Breath::default();
    };
    if !fire_resistant && in_lava {
        return Breath::default();
    }
    if can_breathe && !in_lava {
        return Breath::default();
    }
    let tanks = with_air(tanks);
    let Some(first) = tanks.first() else {
        return Breath::default();
    };
    if in_lava && !tanks.iter().any(|t| t.fire_resistant) {
        return Breath::default();
    }
    Breath {
        consume: (game_time % 20 == 0).then_some(first.slot),
        refill: !in_lava,
    }
}

/// What `DivingBootsItem.accelerateDescentUnderwater` reads of the wearer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Diver {
    pub motion: [f64; 3],
    pub jumping: bool,
    pub on_ground: bool,
    pub vertical_collision: bool,
    /// `zza` and `xxa`: forward and sideways input.
    pub zza: f32,
    pub xxa: f32,
    pub shift: bool,
}

/// `DivingBootsItem.affects`: worn, in water, not swimming, not flying.
pub fn boots_affect(worn: bool, in_water: bool, swimming: bool, flying: bool) -> bool {
    worn && in_water && !swimming && !flying
}

/// `DivingBootsItem.accelerateDescentUnderwater`: the new motion and whether on the ground.
pub fn accelerate_descent(d: &Diver) -> ([f64; 3], bool) {
    let mut motion = d.motion;
    let mut on_ground = d.on_ground || d.vertical_collision;
    if d.jumping && on_ground {
        motion[1] += f64::from(0.5f32);
        on_ground = false;
    } else {
        motion[1] += f64::from(-0.05f32);
    }
    let multiplier = f64::from(1.3f32);
    let horizontal = (motion[0] * motion[0] + motion[2] * motion[2]).sqrt();
    if horizontal < f64::from(0.145f32) && (d.zza > 0.0 || d.xxa != 0.0) && !d.shift {
        motion[0] *= multiplier;
        motion[2] *= multiplier;
    }
    (motion, on_ground)
}

/// `ExtendoGripItem.singleRangeAttributeModifier` / `doubleRangeAttributeModifier`, on
/// `BLOCK_INTERACTION_RANGE`.
pub const SINGLE_RANGE: (&str, f64) = ("create:single_range_attribute_modifier", 3.0);
pub const DOUBLE_RANGE: (&str, f64) = ("create:double_range_attribute_modifier", 5.0);

/// `holdingExtendoGripIncreasesRange`: (one grip, two grips) for grips in the main and off hand.
pub fn extendo_reach(main: bool, off: bool) -> (bool, bool) {
    let dual = main && off;
    ((main ^ off) && !dual, dual)
}

/// Create's wearable and held equipment, by item id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gear {
    Other,
    /// A diving helmet; `true` for the netherite one (fire resistant).
    Helmet(bool),
    /// A backtank (`#create:pressurized_air_sources`); `true` for the netherite one.
    Backtank(bool),
    /// Diving boots; `true` for the netherite ones.
    Boots(bool),
    ExtendoGrip,
}

impl Gear {
    /// The gear an item id is. A string match: the glue caches it per item, so a tick
    /// compares pointers only.
    pub fn of(item: &str) -> Gear {
        match item {
            "create:copper_diving_helmet" => Gear::Helmet(false),
            "create:netherite_diving_helmet" => Gear::Helmet(true),
            "create:copper_backtank" => Gear::Backtank(false),
            "create:netherite_backtank" => Gear::Backtank(true),
            "create:copper_diving_boots" => Gear::Boots(false),
            "create:netherite_diving_boots" => Gear::Boots(true),
            "create:extendo_grip" => Gear::ExtendoGrip,
            _ => Gear::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill_ticks(speed: f32) -> (u32, Vec<i32>) {
        let mut b = Backtank::default();
        let mut ticks = 0;
        let mut levels = Vec::new();
        while b.air < b.max_air() {
            ticks += 1;
            if b.tick(speed, false).is_some() {
                levels.push(b.air);
            }
            assert!(ticks < 100_000);
        }
        (ticks, levels)
    }

    #[test]
    fn air_per_fill_and_wait_follow_the_speed() {
        // ((int) abs - 100) / 20, clamped to 1..5; (int) (128 - abs / 5) - 108, clamped to 0..20.
        assert_eq!((fill_increment(16.0), fill_delay(16.0)), (1, 16));
        assert_eq!((fill_increment(-16.0), fill_delay(-16.0)), (1, 16));
        assert_eq!((fill_increment(64.0), fill_delay(64.0)), (1, 7));
        assert_eq!((fill_increment(128.0), fill_delay(128.0)), (1, 0));
        assert_eq!((fill_increment(160.0), fill_delay(160.0)), (3, 0));
        assert_eq!((fill_increment(256.0), fill_delay(256.0)), (5, 0));
        assert_eq!((fill_increment(1.0), fill_delay(1.0)), (1, 19));
    }

    #[test]
    fn a_backtank_fills_to_900_at_create_s_rates() {
        // 256 rpm: 5 air a tick, 180 ticks.
        let (ticks, levels) = fill_ticks(256.0);
        assert_eq!((ticks, levels[0], levels.len()), (180, 5, 180));
        // 16 rpm: 1 air, then 16 ticks waiting: 17 ticks a point.
        let (ticks, levels) = fill_ticks(16.0);
        assert_eq!(levels[..3], [1, 2, 3]);
        assert_eq!(ticks, 1 + 899 * 17);
        // 128 rpm: 1 a tick.
        assert_eq!(fill_ticks(128.0).0, 900);
        // Capacity II holds 900 + 2 * 300.
        let mut b = Backtank {
            capacity: 2,
            ..Backtank::default()
        };
        assert_eq!(b.max_air(), 1500);
        for _ in 0..400 {
            b.tick(256.0, false);
        }
        assert_eq!(b.air, 1500);
    }

    #[test]
    fn a_backtank_waits_without_speed_or_in_water_and_stops_when_full() {
        let mut b = Backtank::default();
        assert_eq!(b.tick(0.0, false), None);
        assert_eq!(b.tick(256.0, true), None);
        assert_eq!(b.air, 0);
        assert!(!b.ticks(0.0, false) && !b.ticks(32.0, true) && b.ticks(32.0, false));
        b.air = 898;
        let f = b.tick(256.0, false).unwrap();
        assert_eq!((b.air, f.full, f.comparator_changed), (900, true, true));
        assert_eq!(b.tick(256.0, false), None);
        assert!(
            !b.ticks(256.0, false),
            "full and not waiting: nothing to do"
        );
    }

    #[test]
    fn the_comparator_reads_the_fill() {
        let at = |air| {
            Backtank {
                air,
                ..Backtank::default()
            }
            .comparator()
        };
        assert_eq!(at(0), 0);
        assert_eq!(at(1), 1);
        assert_eq!(at(450), 8);
        assert_eq!(at(899), 14);
        assert_eq!(at(900), 15);
    }

    #[test]
    fn air_drains_one_a_second_under_water_with_warnings() {
        let tank = Tank {
            slot: 2,
            stored: 900,
            capacity: 0,
            fire_resistant: false,
        };
        // Under water (can't breathe): one air every 20 ticks, and breathing is refilled.
        let mut stored = tank.stored;
        for t in 0..200i64 {
            let b = breathe(Some(false), false, false, &[Tank { stored, ..tank }], t);
            assert!(b.refill);
            if let Some(slot) = b.consume {
                assert_eq!(slot, 2);
                stored = consume_air(stored, 0, 1).0;
            }
        }
        assert_eq!(stored, 890);
        // Above water: nothing.
        assert_eq!(
            breathe(Some(false), false, true, &[tank], 0),
            Breath::default()
        );
        // No helmet, or an empty tank: nothing.
        assert_eq!(breathe(None, false, false, &[tank], 0), Breath::default());
        let empty = Tank { stored: 0, ..tank };
        assert_eq!(
            breathe(Some(false), false, false, &[empty], 0),
            Breath::default()
        );
        // The tenth and the last air warn once each.
        assert_eq!(consume_air(91, 0, 1), (90, vec![Warning::Low]));
        assert_eq!(consume_air(90, 0, 1), (89, vec![]));
        assert_eq!(consume_air(2, 0, 1), (1, vec![Warning::Depleted]));
        assert_eq!(consume_air(1, 0, 1), (0, vec![]));
        assert_eq!(consume_air(0, 0, 1), (0, vec![]));
        // More stored than the maximum counts as the maximum.
        assert_eq!(consume_air(5000, 0, 1).0, 899);
    }

    #[test]
    fn the_emptiest_tank_is_used_first() {
        let t = |slot, stored| Tank {
            slot,
            stored,
            capacity: 0,
            fire_resistant: false,
        };
        let b = breathe(
            Some(false),
            false,
            false,
            &[t(2, 500), t(0, 30), t(1, 0)],
            40,
        );
        assert_eq!(b.consume, Some(0));
    }

    #[test]
    fn lava_needs_netherite_and_only_drains() {
        let copper = Tank {
            slot: 2,
            stored: 900,
            capacity: 0,
            fire_resistant: false,
        };
        let netherite = Tank {
            fire_resistant: true,
            ..copper
        };
        assert_eq!(
            breathe(Some(false), true, false, &[netherite], 0),
            Breath::default()
        );
        assert_eq!(
            breathe(Some(true), true, false, &[copper], 0),
            Breath::default()
        );
        let b = breathe(Some(true), true, true, &[netherite], 0);
        assert_eq!(
            b,
            Breath {
                consume: Some(2),
                refill: false
            }
        );
        assert_eq!(
            breathe(Some(true), true, true, &[netherite], 1).consume,
            None
        );
    }

    #[test]
    fn boots_sink_and_jump_off_the_floor() {
        let d = Diver {
            motion: [0.0, 0.0, 0.0],
            jumping: false,
            on_ground: false,
            vertical_collision: false,
            zza: 0.0,
            xxa: 0.0,
            shift: false,
        };
        let (m, ground) = accelerate_descent(&d);
        assert_eq!((m[1], ground), (f64::from(-0.05f32), false));
        let (m, ground) = accelerate_descent(&Diver {
            jumping: true,
            vertical_collision: true,
            ..d
        });
        assert_eq!((m[1], ground), (0.5, false));
        // Walking slowly forward speeds up by 1.3; faster or sneaking does not.
        let walk = Diver {
            motion: [0.1, 0.0, 0.0],
            zza: 1.0,
            ..d
        };
        assert_eq!(accelerate_descent(&walk).0[0], 0.1 * f64::from(1.3f32));
        let fast = Diver {
            motion: [0.2, 0.0, 0.0],
            ..walk
        };
        assert_eq!(accelerate_descent(&fast).0[0], 0.2);
        let sneak = Diver {
            shift: true,
            ..walk
        };
        assert_eq!(accelerate_descent(&sneak).0[0], 0.1);
        assert!(boots_affect(true, true, false, false));
        assert!(!boots_affect(true, true, true, false) && !boots_affect(true, false, false, false));
        assert!(!boots_affect(true, true, false, true) && !boots_affect(false, true, false, false));
    }

    #[test]
    fn grips_reach_three_further_or_five_with_two() {
        assert_eq!(extendo_reach(false, false), (false, false));
        assert_eq!(extendo_reach(true, false), (true, false));
        assert_eq!(extendo_reach(false, true), (true, false));
        assert_eq!(extendo_reach(true, true), (false, true));
        // One full copper tank pays for 1000 actions: one air each.
        assert_eq!(absorb_cost(MAX_EXTENDO_GRIP_ACTIONS), 1);
        assert_eq!(absorb_cost(200), 4);
    }

    #[test]
    fn gear_is_known_by_id() {
        assert_eq!(
            Gear::of("create:netherite_diving_helmet"),
            Gear::Helmet(true)
        );
        assert_eq!(Gear::of("create:copper_backtank"), Gear::Backtank(false));
        assert_eq!(Gear::of("create:copper_diving_boots"), Gear::Boots(false));
        assert_eq!(Gear::of("create:extendo_grip"), Gear::ExtendoGrip);
        assert_eq!(Gear::of("minecraft:iron_helmet"), Gear::Other);
    }
}
