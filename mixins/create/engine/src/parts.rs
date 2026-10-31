//! A Create block state as kinetic parts: each block class's `hasShaftTowards`, `getRotationAxis`,
//! cog type, generated speed and stress, decoded once per state when a world opens.

use crate::Block;
use crate::kinetics::{Axis, Dir, Part, convert_to_direction};

/// What the kinetic rules read of one block state, decoded from its property strings once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StateInfo {
    pub axis: Option<Axis>,
    pub facing: Option<Dir>,
    pub powered: bool,
    /// Encased cogwheels' `top_shaft` / `bottom_shaft`.
    pub top_shaft: bool,
    pub bottom_shaft: bool,
}

impl StateInfo {
    /// Reads the properties the kinetic rules use; others are ignored.
    pub fn read<'a>(props: impl Iterator<Item = (&'a str, &'a str)>) -> StateInfo {
        let mut s = StateInfo::default();
        for (k, v) in props {
            match k {
                "axis" => {
                    s.axis = match v {
                        "x" => Some(Axis::X),
                        "y" => Some(Axis::Y),
                        "z" => Some(Axis::Z),
                        _ => None,
                    }
                }
                "facing" => {
                    s.facing = match v {
                        "down" => Some(Dir::Down),
                        "up" => Some(Dir::Up),
                        "north" => Some(Dir::North),
                        "south" => Some(Dir::South),
                        "west" => Some(Dir::West),
                        "east" => Some(Dir::East),
                        _ => None,
                    }
                }
                "powered" => s.powered = v == "true",
                "top_shaft" => s.top_shaft = v == "true",
                "bottom_shaft" => s.bottom_shaft = v == "true",
                _ => {}
            }
        }
        s
    }
}

/// The block's settings a part needs that its state does not hold (a block entity's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// A creative motor's speed setting (CreativeMotorBlockEntity.DEFAULT_SPEED is 16).
    pub motor_rpm: f32,
    /// A hand crank being turned: 1, -1 backwards (sneaking), 0 at rest.
    pub crank: i8,
    /// A water wheel's flow score (`WaterWheelBlockEntity.flowScore`).
    pub flow: i8,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            motor_rpm: 16.0,
            crank: 0,
            flow: 0,
        }
    }
}

/// `HandCrankBlock.getRotationSpeed`.
pub const CRANK_RPM: f32 = 32.0;

/// The kinetic parts of a block state: none for a block that is not kinetic, two for a split shaft
/// (its halves, the second linked to the first).
pub fn parts(block: Block, s: &StateInfo, settings: &Settings) -> [Option<Part>; 2] {
    let with_stress = |p: Part| Part {
        impact: block.impact(),
        capacity: block.capacity(),
        ..p
    };
    let one = |p: Part| [Some(with_stress(p)), None];
    match block {
        Block::Shaft | Block::PoweredShaft => s.axis.map_or([None, None], |a| one(Part::shaft(a))),
        Block::Cogwheel => s.axis.map_or([None, None], |a| one(Part::cog(a, false))),
        Block::LargeCogwheel => s.axis.map_or([None, None], |a| one(Part::cog(a, true))),
        Block::AndesiteEncasedShaft | Block::BrassEncasedShaft | Block::MetalGirderEncasedShaft => {
            s.axis.map_or([None, None], |a| one(Part::shaft(a)))
        }
        Block::AndesiteEncasedCogwheel
        | Block::BrassEncasedCogwheel
        | Block::AndesiteEncasedLargeCogwheel
        | Block::BrassEncasedLargeCogwheel => s.axis.map_or([None, None], |a| {
            // EncasedCogwheelBlock.hasShaftTowards: along the axis, where top/bottom_shaft is set.
            let large = matches!(
                block,
                Block::AndesiteEncasedLargeCogwheel | Block::BrassEncasedLargeCogwheel
            );
            let mut p = Part::cog(a, large);
            p.shafts = 0;
            if s.top_shaft {
                p.shafts |= Dir::from_axis(a, true).bit();
            }
            if s.bottom_shaft {
                p.shafts |= Dir::from_axis(a, false).bit();
            }
            one(p)
        }),
        Block::Gearbox => s.axis.map_or([None, None], |a| one(Part::gearbox(a))),
        Block::Clutch | Block::Gearshift => s.axis.map_or([None, None], |a| {
            // SplitShaftBlockEntity: the half away from the source turns at 0 (clutch) or reversed
            // (gearshift) while powered.
            let link = match (block, s.powered) {
                (_, false) => 1.0,
                (Block::Clutch, true) => 0.0,
                _ => -1.0,
            };
            let first = Part::half(Dir::from_axis(a, false));
            let second = Part {
                link: Some(link),
                ..Part::half(Dir::from_axis(a, true))
            };
            [Some(with_stress(first)), Some(with_stress(second))]
        }),
        Block::CreativeMotor => s.facing.map_or([None, None], |f| {
            // CreativeMotorBlock.hasShaftTowards: its facing; getGeneratedSpeed converts the
            // setting to that face's direction.
            one(Part {
                generated: convert_to_direction(settings.motor_rpm, f),
                ..Part::half(f)
            })
        }),
        Block::HandCrank => s.facing.map_or([None, None], |f| {
            // HandCrankBlock.hasShaftTowards: the face it hangs from; HandCrankBlockEntity
            // .getGeneratedSpeed: 32 while turned (negated backwards), in its facing's direction.
            one(Part {
                generated: convert_to_direction(f32::from(settings.crank) * CRANK_RPM, f),
                ..Part::half(f.opposite())
            })
        }),
        // MillstoneBlock: a small cog on the y axis with a shaft below.
        Block::Millstone => one(Part {
            shafts: Dir::Down.bit(),
            ..Part::cog(Axis::Y, false)
        }),
        Block::WaterWheel => s.facing.map_or([None, None], |f| {
            // WaterWheelBlock.hasShaftTowards: both ends of its axis.
            one(Part {
                generated: crate::waterwheel::generated_speed(i32::from(settings.flow), 1),
                ..Part::shaft(f.axis())
            })
        }),
        _ => [None, None],
    }
}

/// Whether a block can hold kinetic parts at all, for the world-open table.
pub fn is_kinetic(block: Block) -> bool {
    let probe = StateInfo {
        axis: Some(Axis::Y),
        facing: Some(Dir::Up),
        top_shaft: true,
        bottom_shaft: true,
        powered: false,
    };
    parts(block, &probe, &Settings::default())[0].is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinetics::{Event, Kinetics};

    fn info(props: &[(&str, &str)]) -> StateInfo {
        StateInfo::read(props.iter().copied())
    }

    #[test]
    fn states_decode_once_into_parts() {
        let s = info(&[("axis", "x"), ("waterlogged", "false")]);
        let [p, none] = parts(Block::Shaft, &s, &Settings::default());
        assert_eq!(p.unwrap().axis, Axis::X);
        assert!(none.is_none());
        assert!(is_kinetic(Block::Cogwheel));
        assert!(!is_kinetic(Block::AndesiteCasing));
    }

    #[test]
    fn a_creative_motor_drives_a_shaft_and_a_press_costs_its_stress() {
        let mut k = Kinetics::new();
        let motor = info(&[("facing", "east")]);
        let [m, _] = parts(
            Block::CreativeMotor,
            &motor,
            &Settings {
                motor_rpm: 32.0,
                ..Settings::default()
            },
        );
        k.set([0, 0, 0], 0, m.unwrap());
        let shaft = info(&[("axis", "x")]);
        k.set(
            [1, 0, 0],
            0,
            parts(Block::Shaft, &shaft, &Settings::default())[0].unwrap(),
        );
        let mut e = Vec::new();
        k.solve(&mut e);
        assert_eq!(k.speed([1, 0, 0], 0), Some(32.0));
        assert!(e.iter().all(|x| !matches!(x, Event::Break { .. })));
    }

    #[test]
    fn a_powered_gearshift_reverses_and_a_powered_clutch_cuts() {
        for (block, expect) in [(Block::Gearshift, -16.0), (Block::Clutch, 0.0)] {
            let mut k = Kinetics::new();
            let motor = info(&[("facing", "east")]);
            k.set(
                [-1, 0, 0],
                0,
                parts(Block::CreativeMotor, &motor, &Settings::default())[0].unwrap(),
            );
            let split = info(&[("axis", "x"), ("powered", "true")]);
            let [a, b] = parts(block, &split, &Settings::default());
            k.set([0, 0, 0], 0, a.unwrap());
            k.set([0, 0, 0], 1, b.unwrap());
            let shaft = info(&[("axis", "x")]);
            k.set(
                [1, 0, 0],
                0,
                parts(Block::Shaft, &shaft, &Settings::default())[0].unwrap(),
            );
            k.solve(&mut Vec::new());
            assert_eq!(k.speed([1, 0, 0], 0), Some(expect), "{block:?}");
        }
    }
}
