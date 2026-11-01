//! Create's boiler and steam engine (`BoilerData`, `BoilerHeaters`, `BoilerHeater`,
//! `SteamEngineBlock`, `SteamEngineBlockEntity`, `PoweredShaftBlock`, `PoweredShaftBlockEntity`):
//! a fluid tank with engines on it is a boiler; its level is the least of its heat, its water
//! supply and its size; each engine drives the powered shaft two blocks in front of it at a speed
//! and stress capacity set by its share of the boiler's level.

use crate::fan::Heat;
use crate::kinetics::{Axis, Dir, convert_to_direction};

/// `BoilerData.SAMPLE_RATE`: the water supply is sampled every 5 ticks.
pub const SAMPLE_RATE: i32 = 5;
/// `BoilerData.waterSupplyPerLevel`: mB a tick per level.
pub const WATER_SUPPLY_PER_LEVEL: i32 = 10;
/// `BoilerData.passiveEngineEfficiency`.
pub const PASSIVE_ENGINE_EFFICIENCY: f32 = 1.0 / 8.0;
/// The highest boiler level (`getMaxHeatLevelForBoilerSize`, `getMaxHeatLevelForWaterSupply`).
pub const MAX_LEVEL: i32 = 18;
/// `BoilerHeater.PASSIVE_HEAT` and `NO_HEAT`.
pub const PASSIVE_HEAT: i32 = 0;
pub const NO_HEAT: i32 = -1;

/// `BoilerHeaters.blazeBurner`: none without a blaze, passive while smouldering, 1 fading or
/// kindled, 2 seething.
pub fn blaze_burner_heat(h: Heat) -> i32 {
    match h {
        Heat::None => NO_HEAT,
        Heat::Seething => 2,
        Heat::Fading | Heat::Kindled => 1,
        Heat::Smouldering => PASSIVE_HEAT,
    }
}

/// `BoilerHeaters.passive` for a block in `create:passive_boiler_heaters` (magma, lava, fire,
/// campfires, blaze burners): passive unless `BlockHelper.isNotUnheated` says it is out (an
/// unlit campfire).
pub fn passive_heat(heated: bool) -> i32 {
    if heated { PASSIVE_HEAT } else { NO_HEAT }
}

/// `BoilerData`: one boiler's heat, water supply and engines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Boiler {
    /// Water filled in since the last sample (`gatheredSupply`).
    pub gathered_supply: u32,
    pub supply_over_time: [f32; 10],
    pub ticks_until_next_sample: i32,
    pub current_index: usize,
    pub needs_heat_level_update: bool,
    pub passive_heat: bool,
    pub active_heat: i32,
    /// mB a tick (`waterSupply`).
    pub water_supply: f32,
    pub attached_engines: i32,
}

impl Boiler {
    /// `isActive` (whistles are not ported).
    pub fn is_active(&self) -> bool {
        self.attached_engines > 0
    }

    /// `clear`: a tank split off or formed again starts without supply, heat or engines.
    pub fn clear(&mut self) {
        self.water_supply = 0.0;
        self.active_heat = 0;
        self.passive_heat = false;
        self.attached_engines = 0;
        self.supply_over_time = [0.0; 10];
    }

    /// `updateTemperature`: the heat of each block under the boiler (`BoilerHeater.findHeat`);
    /// true when it changed.
    pub fn update_temperature(&mut self, heats: impl IntoIterator<Item = i32>) -> bool {
        self.needs_heat_level_update = false;
        let (prev_passive, prev_active) = (self.passive_heat, self.active_heat);
        self.passive_heat = false;
        self.active_heat = 0;
        for heat in heats {
            if heat == PASSIVE_HEAT {
                self.passive_heat = true;
            } else if heat > 0 {
                self.active_heat += heat;
            }
        }
        self.passive_heat &= self.active_heat == 0;
        prev_active != self.active_heat || prev_passive != self.passive_heat
    }

    /// `tick` on the server, after its heat was updated: every `SAMPLE_RATE` ticks the water
    /// gathered becomes a sample, and the supply the highest of the last ten samples (raised at
    /// once, lowered once the ten went round). True when it sampled (`notifyUpdate`).
    pub fn tick(&mut self, capacity: u32, creative: bool) -> bool {
        if !self.is_active() {
            return false;
        }
        self.ticks_until_next_sample -= 1;
        if self.ticks_until_next_sample > 0 || capacity == 0 {
            return false;
        }
        self.ticks_until_next_sample = SAMPLE_RATE;
        let sample = self.gathered_supply as f32 / SAMPLE_RATE as f32;
        self.supply_over_time[self.current_index] = sample;
        self.water_supply = self.water_supply.max(sample);
        self.current_index = (self.current_index + 1) % self.supply_over_time.len();
        self.gathered_supply = 0;
        if self.current_index == 0 {
            self.water_supply = 0.0;
            for i in self.supply_over_time {
                self.water_supply = i.max(self.water_supply);
            }
        }
        if creative {
            self.water_supply = (WATER_SUPPLY_PER_LEVEL * 20) as f32;
        }
        true
    }

    /// `BoilerFluidHandler.fill`: water is all taken, as supply; nothing else is.
    pub fn fill(&mut self, water: bool, amount: u32) -> u32 {
        if !water {
            return 0;
        }
        self.gathered_supply += amount;
        amount
    }

    /// `getMaxHeatLevelForWaterSupply`.
    pub fn max_heat_for_water(&self) -> i32 {
        MAX_LEVEL.min(self.water_supply.ceil() as i32 / WATER_SUPPLY_PER_LEVEL)
    }

    /// `isPassive(boilerSize)`.
    pub fn is_passive(&self, size: i32) -> bool {
        self.passive_heat && max_heat_for_size(size) > 0 && self.max_heat_for_water() > 0
    }

    /// `getActualHeat`: the boiler's level, the least of heat, water and size.
    pub fn level(&self, size: i32) -> i32 {
        self.active_heat
            .min(self.max_heat_for_water().min(max_heat_for_size(size)))
    }

    /// `getEngineEfficiency`: each engine's share, 1 while there are no more engines than
    /// levels.
    pub fn engine_efficiency(&self, size: i32) -> f32 {
        if self.is_passive(size) {
            return PASSIVE_ENGINE_EFFICIENCY / self.attached_engines as f32;
        }
        if self.active_heat == 0 {
            return 0.0;
        }
        let actual = self.level(size);
        if self.attached_engines <= actual {
            1.0
        } else {
            actual as f32 / self.attached_engines as f32
        }
    }
}

/// `getMaxHeatLevelForBoilerSize`: a level per 4 blocks of tank.
pub fn max_heat_for_size(size: i32) -> i32 {
    MAX_LEVEL.min(size / 4)
}

/// `FaceAttachedHorizontalDirectionalBlock.FACE` (`face`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Floor,
    Wall,
    Ceiling,
}

impl Face {
    pub fn read(v: &str) -> Option<Face> {
        Some(match v {
            "floor" => Face::Floor,
            "wall" => Face::Wall,
            "ceiling" => Face::Ceiling,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Face::Floor => "floor",
            Face::Wall => "wall",
            Face::Ceiling => "ceiling",
        }
    }
}

/// `getConnectedDirection` (`SteamEngineBlock.getFacing`): the way the engine points, away from
/// its tank: up on the floor, down on the ceiling, its `facing` on a wall.
pub fn connected_direction(face: Face, facing: Dir) -> Dir {
    match face {
        Face::Floor => Dir::Up,
        Face::Ceiling => Dir::Down,
        Face::Wall => facing,
    }
}

/// `FaceAttachedHorizontalDirectionalBlock.getStateForPlacement` with `SteamEngineBlock
/// .canSurvive`: the first of the directions nearest the player's view (`looking`) with a tank
/// that way; up or down puts it on the ceiling or floor turned to the player's `horizontal`
/// direction, a side on a wall facing back. None: it can't be placed.
pub fn placement(
    looking: [Dir; 6],
    horizontal: Dir,
    tank: impl Fn(Dir) -> bool,
) -> Option<(Face, Dir)> {
    looking.into_iter().find(|d| tank(*d)).map(|d| match d {
        Dir::Up => (Face::Ceiling, horizontal),
        Dir::Down => (Face::Floor, horizontal),
        d => (Face::Wall, d.opposite()),
    })
}

/// `SteamEngineBlock.getShaftPos`: two blocks the way the engine points.
pub fn shaft_pos(engine: [i32; 3], connected: Dir) -> [i32; 3] {
    let o = connected.offset();
    [
        engine[0] + 2 * o[0],
        engine[1] + 2 * o[1],
        engine[2] + 2 * o[2],
    ]
}

/// `SteamEngineBlock.isShaftValid` for a shaft or powered shaft: across the engine.
pub fn is_shaft_valid(connected: Dir, shaft_axis: Axis) -> bool {
    shaft_axis != connected.axis()
}

/// `PoweredShaftBlock.stillValid`: an engine two blocks away on a side across the shaft points
/// at it. `engine_at` gives an engine's connected direction.
pub fn still_valid(
    shaft: [i32; 3],
    axis: Axis,
    engine_at: impl Fn([i32; 3]) -> Option<Dir>,
) -> bool {
    Dir::ALL.into_iter().filter(|d| d.axis() != axis).any(|d| {
        let engine = shaft_pos(shaft, d);
        engine_at(engine).is_some_and(|c| c == d.opposite() && is_shaft_valid(c, axis))
    })
}

/// `BoilerData.evaluate` for the tank blocks `blocks`: the engines on them pointing away.
pub fn attached_engines(
    blocks: impl IntoIterator<Item = [i32; 3]>,
    engine_at: impl Fn([i32; 3]) -> Option<Dir>,
) -> Vec<[i32; 3]> {
    let mut out = Vec::new();
    for p in blocks {
        for d in Dir::ALL {
            let o = d.offset();
            let at = [p[0] + o[0], p[1] + o[1], p[2] + o[2]];
            if engine_at(at) == Some(d) {
                out.push(at);
            }
        }
    }
    out
}

/// `SteamEngineBlockEntity.tick`: the direction an engine turns its shaft (clockwise unless
/// `counter_clockwise`, its scroll option), from the way it points (`facing` when that is
/// up or down) and the shaft's axis.
pub fn conveyed_direction(
    efficiency: f32,
    connected: Dir,
    facing: Dir,
    target_axis: Axis,
    counter_clockwise: bool,
) -> i8 {
    let facing = if connected.axis() == Axis::Y {
        facing
    } else {
        connected
    };
    let mut level = if efficiency == 0.0 || target_axis == Axis::Y {
        1
    } else {
        convert_to_direction(1.0, facing) as i8
    };
    if target_axis == Axis::Z {
        level = -level;
    }
    if counter_clockwise {
        level = -level;
    }
    level
}

/// What drives a powered shaft (`PoweredShaftBlockEntity`): its engine's direction and
/// efficiency. Nothing drives it at efficiency 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Output {
    pub direction: i8,
    pub efficiency: f32,
}

impl Default for Output {
    /// The constructor's `movementDirection` 1, no engine.
    fn default() -> Self {
        Output {
            direction: 1,
            efficiency: 0.0,
        }
    }
}

impl Output {
    /// `PoweredShaftBlockEntity.update`'s test: whether it changed (`Mth.equal`).
    pub fn differs(&self, o: &Output) -> bool {
        (self.efficiency - o.efficiency).abs() >= 1.0e-5 || self.direction != o.direction
    }

    /// `getSpeedModifier`: 1 to 4 by quarters of efficiency, 4 at full.
    pub fn speed_modifier(&self) -> i32 {
        1 + if self.efficiency >= 1.0 {
            3
        } else {
            ((self.efficiency * 4.0).floor() as i32).min(2)
        }
    }

    /// `getCombinedCapacity`: the engine's capacity (`CStress`, 1024) times its efficiency.
    fn combined_capacity(&self) -> f32 {
        self.efficiency * crate::Block::SteamEngine.capacity()
    }

    /// `getGeneratedSpeed`: 16 RPM per speed step, in its direction.
    pub fn generated_speed(&self) -> f32 {
        if self.combined_capacity() > 0.0 {
            f32::from(self.direction) * 16.0 * self.speed_modifier() as f32
        } else {
            0.0
        }
    }

    /// `calculateAddedStressCapacity`: SU per RPM, the capacity shared over the speed steps.
    pub fn capacity(&self) -> f32 {
        self.combined_capacity() / self.speed_modifier() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(efficiency: f32) -> (f32, f32) {
        let o = Output {
            direction: 1,
            efficiency,
        };
        (o.generated_speed(), o.capacity() * o.generated_speed())
    }

    #[test]
    fn heaters_give_create_s_heat() {
        assert_eq!(blaze_burner_heat(Heat::None), NO_HEAT);
        assert_eq!(blaze_burner_heat(Heat::Smouldering), PASSIVE_HEAT);
        assert_eq!(blaze_burner_heat(Heat::Fading), 1);
        assert_eq!(blaze_burner_heat(Heat::Kindled), 1);
        assert_eq!(blaze_burner_heat(Heat::Seething), 2);
        assert_eq!(passive_heat(false), NO_HEAT);
        let mut b = Boiler::default();
        assert!(b.update_temperature([PASSIVE_HEAT]));
        assert!((b.passive_heat, b.active_heat) == (true, 0));
        // Any active heater makes the boiler active, not passive.
        assert!(b.update_temperature([PASSIVE_HEAT, 2, 1, NO_HEAT]));
        assert!((b.passive_heat, b.active_heat) == (false, 3));
        assert!(!b.update_temperature([1, 2]), "same heat");
    }

    #[test]
    fn a_level_per_four_blocks_and_ten_mb_a_tick() {
        assert_eq!(max_heat_for_size(1), 0, "a one-block tank is no boiler");
        assert_eq!(max_heat_for_size(4), 1);
        assert_eq!(max_heat_for_size(27), 6);
        assert_eq!(max_heat_for_size(72), 18);
        assert_eq!(max_heat_for_size(81), 18);
        let water = |w: f32| {
            Boiler {
                water_supply: w,
                ..Boiler::default()
            }
            .max_heat_for_water()
        };
        assert_eq!(water(9.0), 0);
        assert_eq!(water(9.5), 1, "rounded up");
        assert_eq!(water(25.0), 2);
        assert_eq!(water(500.0), 18);
    }

    #[test]
    fn the_supply_is_the_best_of_ten_samples() {
        let mut b = Boiler {
            attached_engines: 1,
            ..Boiler::default()
        };
        // A pump at 16 RPM gives 8 mB a tick: 40 mB a sample, 8 mB a tick of supply.
        b.fill(true, 40);
        assert_eq!(b.fill(false, 40), 0, "only water");
        assert!(b.tick(8000, false), "the first tick samples");
        assert_eq!(b.water_supply, 8.0);
        assert_eq!(b.max_heat_for_water(), 0);
        for _ in 0..4 {
            b.fill(true, 15);
            assert!(!b.tick(8000, false));
        }
        b.fill(true, 15);
        assert!(b.tick(8000, false));
        assert_eq!(b.water_supply, 15.0, "raised at once");
        assert_eq!(b.max_heat_for_water(), 1);
        // Dry: the supply stays until the ten samples went round.
        for _ in 0..7 * SAMPLE_RATE {
            b.tick(8000, false);
        }
        assert_eq!((b.current_index, b.water_supply), (9, 15.0));
        for _ in 0..SAMPLE_RATE {
            b.tick(8000, false);
        }
        assert_eq!((b.current_index, b.water_supply), (0, 15.0));
        for _ in 0..10 * SAMPLE_RATE {
            b.tick(8000, false);
        }
        assert_eq!(b.water_supply, 0.0);
        let mut idle = Boiler::default();
        assert!(!idle.tick(8000, false), "without engines it does nothing");
        b.ticks_until_next_sample = 0;
        b.tick(8000, true);
        assert_eq!(b.water_supply, 200.0, "a creative tank");
    }

    #[test]
    fn engines_share_the_boiler_s_level() {
        let boiler = |heat: i32, passive: bool, water: f32, engines: i32| Boiler {
            active_heat: heat,
            passive_heat: passive,
            water_supply: water,
            attached_engines: engines,
            ..Boiler::default()
        };
        // 2x2x2 tank (size 8, level 2 at most), a seething burner, 20 mB a tick.
        assert_eq!(boiler(2, false, 20.0, 1).level(8), 2);
        assert_eq!(boiler(2, false, 20.0, 2).engine_efficiency(8), 1.0);
        assert_eq!(boiler(2, false, 20.0, 4).engine_efficiency(8), 0.5);
        assert_eq!(boiler(2, false, 10.0, 4).engine_efficiency(8), 0.25);
        assert_eq!(boiler(4, false, 40.0, 1).level(8), 2, "size limits it");
        assert_eq!(boiler(2, false, 20.0, 1).engine_efficiency(1), 0.0);
        assert_eq!(boiler(0, false, 20.0, 1).engine_efficiency(8), 0.0);
        // Passive: an eighth an engine, with size and water for a level.
        assert!(boiler(0, true, 10.0, 1).is_passive(4));
        assert_eq!(boiler(0, true, 10.0, 1).engine_efficiency(4), 0.125);
        assert_eq!(boiler(0, true, 10.0, 2).engine_efficiency(4), 0.0625);
        assert_eq!(boiler(0, true, 5.0, 1).engine_efficiency(4), 0.0);
        assert_eq!(boiler(0, true, 10.0, 1).engine_efficiency(1), 0.0);
    }

    #[test]
    fn the_powered_shaft_turns_by_quarters_of_efficiency() {
        assert_eq!(crate::Block::SteamEngine.capacity(), 1024.0);
        // Full: 64 RPM and 16,384 SU.
        assert_eq!(output(1.0), (64.0, 16384.0));
        // `getSpeedRange` 16..64: half is 48 RPM, a quarter 32, passive 16.
        let (rpm, su) = output(0.5);
        assert_eq!(rpm, 48.0);
        assert!((su - 8192.0).abs() < 0.01);
        assert_eq!(output(0.25), (32.0, 4096.0));
        assert_eq!(output(0.125), (16.0, 2048.0));
        assert_eq!(output(0.0625), (16.0, 1024.0));
        assert_eq!(output(0.0), (0.0, 0.0));
        let back = Output {
            direction: -1,
            efficiency: 1.0,
        };
        assert_eq!(back.generated_speed(), -64.0);
        assert!(!Output::default().differs(&Output {
            direction: 1,
            efficiency: 0.000_001
        }));
        assert!(Output::default().differs(&back));
    }

    #[test]
    fn engines_turn_their_shaft_by_the_way_they_point() {
        // On a wall facing east, an engine turns a y shaft clockwise (1) and a z shaft -1.
        assert_eq!(
            conveyed_direction(1.0, Dir::East, Dir::East, Axis::Y, false),
            1
        );
        assert_eq!(
            conveyed_direction(1.0, Dir::East, Dir::East, Axis::Z, false),
            -1
        );
        assert_eq!(
            conveyed_direction(1.0, Dir::West, Dir::West, Axis::Z, false),
            1
        );
        assert_eq!(
            conveyed_direction(1.0, Dir::West, Dir::West, Axis::Z, true),
            -1
        );
        // On the floor its horizontal facing counts.
        assert_eq!(
            conveyed_direction(1.0, Dir::Up, Dir::North, Axis::X, false),
            -1
        );
        assert_eq!(
            conveyed_direction(1.0, Dir::Up, Dir::South, Axis::X, false),
            1
        );
        // Without steam: 1, as Create sends it (nothing turns at efficiency 0).
        assert_eq!(
            conveyed_direction(0.0, Dir::Up, Dir::North, Axis::X, false),
            1
        );
    }

    #[test]
    fn engines_sit_on_tanks_and_power_the_shaft_in_front() {
        use crate::place::ordered_by_nearest;
        // Looking down at a tank: on its top, turned the player's way.
        let down = ordered_by_nearest(0.0, 80.0);
        assert_eq!(
            placement(down, Dir::South, |d| d == Dir::Down),
            Some((Face::Floor, Dir::South))
        );
        // Looking north at a tank's side: on the wall, facing back south.
        let north = ordered_by_nearest(180.0, 0.0);
        assert_eq!(
            placement(north, Dir::North, |d| d == Dir::North),
            Some((Face::Wall, Dir::South))
        );
        assert_eq!(
            placement(north, Dir::North, |d| d == Dir::Up),
            Some((Face::Ceiling, Dir::North))
        );
        assert_eq!(placement(north, Dir::North, |_| false), None);
        assert_eq!(connected_direction(Face::Wall, Dir::South), Dir::South);
        assert_eq!(connected_direction(Face::Floor, Dir::South), Dir::Up);
        assert_eq!(shaft_pos([0, 0, 0], Dir::South), [0, 0, 2]);
        // A south-pointing engine at the origin powers an x or y shaft at z 2, not a z one.
        let engine = |p: [i32; 3]| (p == [0, 0, 0]).then_some(Dir::South);
        assert!(still_valid([0, 0, 2], Axis::X, engine));
        assert!(still_valid([0, 0, 2], Axis::Y, engine));
        assert!(!still_valid([0, 0, 2], Axis::Z, engine));
        assert!(!still_valid([0, 0, 3], Axis::X, engine));
        // The tank north of it counts it; one beside it doesn't.
        assert_eq!(attached_engines([[0, 0, -1]], engine), vec![[0, 0, 0]]);
        assert!(attached_engines([[1, 0, 0]], engine).is_empty());
    }
}
