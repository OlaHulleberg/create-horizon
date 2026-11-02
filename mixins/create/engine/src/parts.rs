//! A Create block state as kinetic parts: each block class's `hasShaftTowards`, `getRotationAxis`,
//! cog type, generated speed and stress, decoded once per state when a world opens.

use crate::Block;
use crate::kinetics::{Axis, Chain, Couple, Dir, Part, convert_to_direction};

/// What the kinetic rules read of one block state, decoded from its property strings once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StateInfo {
    pub axis: Option<Axis>,
    pub facing: Option<Dir>,
    pub powered: bool,
    /// Encased cogwheels' `top_shaft` / `bottom_shaft`.
    pub top_shaft: bool,
    pub bottom_shaft: bool,
    /// A belt segment's `facing`, `slope` and `part`.
    pub belt: Option<crate::belt::Segment>,
    /// A latch's `powering` and `powered_side`.
    pub powering: bool,
    pub powered_side: bool,
    /// `DirectionalAxisKineticBlock.AXIS_ALONG_FIRST_COORDINATE`.
    pub along_first: bool,
    /// A blaze burner's `blaze` (heat level).
    pub heat: Option<crate::fan::Heat>,
    /// A redstone link's `receiver`.
    pub receiver: bool,
    /// A brass diode's `inverted`; a rose quartz lamp's `activate`.
    pub inverted: bool,
    pub activate: bool,
    /// A belt tunnel's `shape`.
    pub tunnel: Option<crate::tunnel::Shape>,
    /// An encased chain drive's `part`.
    pub chain: Option<crate::chain::Part>,
    /// A sequenced gearshift's `vertical` and `state` (0 idle, else its instruction + 1).
    pub vertical: bool,
    pub sequence: u8,
    /// A steam engine's or an analog lever's `face`.
    pub face: Option<crate::steam::Face>,
    /// A smart observer's or a threshold switch's `target` (`DirectedDirectionalBlock.TARGET`).
    pub target: Option<crate::steam::Face>,
    /// A threshold switch's `level` (0 to 5).
    pub level: u8,
    /// A train track's `shape`.
    pub track: Option<crate::track::TrackShape>,
    /// An item vault's `large`.
    pub large: bool,
    /// `waterlogged` (a backtank fills only out of water).
    pub waterlogged: bool,
    /// An elevator contact's `calling`.
    pub calling: bool,
    /// A mechanical arm's `ceiling`; a funnel's `extracting`; a belt funnel's `shape`.
    pub ceiling: bool,
    pub extracting: bool,
    pub belt_funnel: Option<crate::funnel::BeltShape>,
    /// A door's or a postbox's `open`, a sliding door's `visible`, `half` (upper) and `hinge`
    /// (right).
    pub open: bool,
    pub visible: bool,
    pub upper: bool,
    pub hinge_right: bool,
    /// A train signal's `type` is `cross_signal`.
    pub cross_signal: bool,
    /// A lit blaze burner's `flame_type` is `soul`.
    pub soul: bool,
    /// A fluid valve's `enabled`.
    pub enabled: bool,
    /// A steam whistle's `wall`; its or an extension's `size`; an extension's `shape`.
    pub wall: bool,
    pub size: Option<crate::whistle::Size>,
    pub extender: Option<crate::whistle::Shape>,
}

impl StateInfo {
    /// Reads the properties the kinetic rules use; others are ignored.
    pub fn read<'a>(props: impl Iterator<Item = (&'a str, &'a str)>) -> StateInfo {
        let mut s = StateInfo::default();
        let props: Vec<(&str, &str)> = props.collect();
        s.belt = crate::belt::Segment::read(props.iter().copied());
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
                "large" => s.large = v == "true",
                "waterlogged" => s.waterlogged = v == "true",
                "calling" => s.calling = v == "true",
                "ceiling" => s.ceiling = v == "true",
                "extracting" => s.extracting = v == "true",
                "powering" => s.powering = v == "true",
                "receiver" => s.receiver = v == "true",
                "inverted" => s.inverted = v == "true",
                "activate" => s.activate = v == "true",
                "shape" => {
                    s.tunnel = crate::tunnel::Shape::read(v);
                    s.track = crate::track::TrackShape::read(v);
                    s.belt_funnel = crate::funnel::BeltShape::read(v);
                    s.extender = crate::whistle::Shape::read(v);
                }
                "enabled" => s.enabled = v == "true",
                "wall" => s.wall = v == "true",
                "size" => s.size = crate::whistle::Size::read(v),
                "part" => s.chain = crate::chain::Part::read(v),
                "vertical" => s.vertical = v == "true",
                "state" => s.sequence = v.parse().unwrap_or(0),
                "face" => s.face = crate::steam::Face::read(v),
                "target" => s.target = crate::steam::Face::read(v),
                "level" => s.level = v.parse().unwrap_or(0),
                "powered_side" => s.powered_side = v == "true",
                "top_shaft" => s.top_shaft = v == "true",
                "bottom_shaft" => s.bottom_shaft = v == "true",
                "open" => s.open = v == "true",
                "visible" => s.visible = v == "true",
                "half" => s.upper = v == "upper",
                "hinge" => s.hinge_right = v == "right",
                "type" => s.cross_signal = v == "cross_signal",
                "flame_type" => s.soul = v == "soul",
                "axis_along_first" => s.along_first = v == "true",
                "blaze" => {
                    use crate::fan::Heat;
                    s.heat = match v {
                        "none" => Some(Heat::None),
                        "smouldering" => Some(Heat::Smouldering),
                        "fading" => Some(Heat::Fading),
                        "kindled" => Some(Heat::Kindled),
                        "seething" => Some(Heat::Seething),
                        _ => None,
                    }
                }
                _ => {}
            }
        }
        s
    }
}

/// The block's settings a part needs that its state does not hold (a block entity's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// A creative motor's speed setting (CreativeMotorBlockEntity.DEFAULT_SPEED is 16), or a
    /// rotation speed controller's target (SpeedControllerBlockEntity.DEFAULT_SPEED, 16 too).
    pub motor_rpm: f32,
    /// A hand crank being turned: 1, -1 backwards (sneaking), 0 at rest.
    pub crank: i8,
    /// A water wheel's flow score (`WaterWheelBlockEntity.flowScore`).
    pub flow: i8,
    /// A belt segment's controller, once its chain is whole (`BeltBlockEntity.controller`).
    pub belt: Option<[i32; 3]>,
    /// A windmill bearing's generated speed while its sails turn (0 at rest).
    pub windmill: f32,
    /// An adjustable chain gearshift's redstone signal (`ChainGearshiftBlockEntity.signal`).
    pub signal: i8,
    /// A sequenced gearshift's far half's speed modifier (`getModifier`: 0 idle).
    pub sequence: i8,
    /// What drives a powered shaft: its steam engine's direction and efficiency.
    pub steam: crate::steam::Output,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            motor_rpm: 16.0,
            crank: 0,
            flow: 0,
            belt: None,
            windmill: 0.0,
            signal: 0,
            sequence: 0,
            steam: crate::steam::Output::default(),
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
        Block::Shaft => s.axis.map_or([None, None], |a| one(Part::shaft(a))),
        // PoweredShaftBlock (AbstractShaftBlock): a shaft its engine turns
        // (`PoweredShaftBlockEntity.getGeneratedSpeed`, `calculateAddedStressCapacity`).
        Block::PoweredShaft => s.axis.map_or([None, None], |a| {
            [
                Some(Part {
                    generated: settings.steam.generated_speed(),
                    capacity: settings.steam.capacity(),
                    ..with_stress(Part::shaft(a))
                }),
                None,
            ]
        }),
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
        // ChainDriveBlock: shafts at both ends of its axis, and its chain to the drives beside it
        // (`areBlocksConnected`), turning it at the adjustable gearshift's modifier for its
        // signal (`ChainGearshiftBlockEntity.getModifier`).
        Block::EncasedChainDrive | Block::AdjustableChainGearshift => {
            s.axis.map_or([None, None], |a| {
                let d = crate::chain::Drive {
                    axis: a,
                    part: s.chain.unwrap_or_default(),
                    along_first: s.along_first,
                };
                let (positive, negative) = d.goes_on();
                let modifier = if block == Block::AdjustableChainGearshift {
                    crate::chain::modifier_for_signal(i32::from(settings.signal))
                } else {
                    1.0
                };
                one(Part {
                    chain: Some(Chain {
                        along: d.connection_axis(),
                        positive,
                        negative,
                        modifier,
                    }),
                    ..Part::shaft(a)
                })
            })
        }
        // SpeedControllerBlock (HorizontalAxisKineticBlock): shafts at both ends of its axis; the
        // large cog above it turns at its target.
        Block::RotationSpeedController => s.axis.map_or([None, None], |a| {
            one(Part {
                couple: Some(Couple::Controller(settings.motor_rpm)),
                ..Part::shaft(a)
            })
        }),
        // SequencedGearshiftBlock: a split shaft on its horizontal axis, or vertical; the half
        // away from the one turned turns at the running instruction's modifier
        // (`SequencedGearshiftBlockEntity.getRotationSpeedModifier`), still while idle.
        Block::SequencedGearshift => s.axis.map_or([None, None], |a| {
            let a = if s.vertical { Axis::Y } else { a };
            let first = Part::half(Dir::from_axis(a, false));
            let second = Part {
                couple: Some(Couple::Relay(f32::from(settings.sequence))),
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
        // ValveHandleBlock (a HandCrankBlock turning at 32 too); every valve handle's stress is
        // the copper one's (`getStressConfigKey`).
        b if crate::dye::is_valve_handle(b) => s.facing.map_or([None, None], |f| {
            [
                Some(Part {
                    generated: convert_to_direction(f32::from(settings.crank) * CRANK_RPM, f),
                    impact: Block::CopperValveHandle.impact(),
                    capacity: Block::CopperValveHandle.capacity(),
                    ..Part::half(f.opposite())
                }),
                None,
            ]
        }),
        // BacktankBlock (HorizontalKineticBlock): turns on y, its shaft out of its top.
        Block::CopperBacktank | Block::NetheriteBacktank => one(Part::half(Dir::Up)),
        // MillstoneBlock: a small cog on the y axis with a shaft below.
        Block::Millstone => one(Part {
            shafts: Dir::Down.bit(),
            ..Part::cog(Axis::Y, false)
        }),
        // BeltBlock: turns on its axis, with a shaft through its pulleys; its segments turn
        // together once their chain is whole.
        Block::Belt => match (s.belt, settings.belt) {
            (Some(seg), Some(controller)) => {
                let axis = seg.rotation_axis();
                let mut p = Part::shaft(axis);
                if !seg.has_pulley() {
                    p.shafts = 0;
                }
                p.belt = Some(controller);
                one(p)
            }
            _ => [None, None],
        },
        // WindmillBearingBlock (BearingBlock): a shaft out of its back; while its sails are
        // assembled it generates `WindmillBearingBlockEntity.getGeneratedSpeed`, not converted
        // to its facing.
        Block::WindmillBearing => s.facing.map_or([None, None], |f| {
            one(Part {
                generated: settings.windmill,
                ..Part::half(f.opposite())
            })
        }),
        // MechanicalCrafterBlock (ICogWheel): a small cog on its facing's axis, no shaft.
        // FlapDisplayBlock (HorizontalKineticBlock, ICogWheel): a small cog on its facing's axis
        // (`getRotationAxis`), no shaft; no stress (`setNoImpact`).
        Block::MechanicalCrafter | Block::DisplayBoard => s.facing.map_or([None, None], |f| {
            one(Part {
                shafts: 0,
                ..Part::cog(f.axis(), false)
            })
        }),
        // GantryShaftBlock (DirectionalKineticBlock): turns on its facing's axis, shafts at both
        // ends.
        Block::GantryShaft => s
            .facing
            .map_or([None, None], |f| one(Part::shaft(f.axis()))),
        // MechanicalBearingBlock and ClockworkBearingBlock (BearingBlock): a shaft out of its
        // back, turning what it carries.
        Block::MechanicalBearing | Block::ClockworkBearing => s
            .facing
            .map_or([None, None], |f| one(Part::half(f.opposite()))),
        // PulleyBlock (HorizontalAxisKineticBlock): shafts at both ends of its axis.
        Block::RopePulley => s.axis.map_or([None, None], |a| one(Part::shaft(a))),
        // ElevatorPulleyBlock (HorizontalKineticBlock): shafts across its facing
        // (`getRotationAxis`: the facing turned clockwise).
        Block::ElevatorPulley => s.facing.map_or([None, None], |f| {
            one(Part::shaft(if f.axis() == Axis::X {
                Axis::Z
            } else {
                Axis::X
            }))
        }),
        // MechanicalPressBlock: turns on its horizontal facing's axis, shafts at both ends.
        Block::MechanicalPress => s
            .facing
            .map_or([None, None], |f| one(Part::shaft(f.axis()))),
        // MechanicalPistonBlock (DirectionalAxisKineticBlock.getRotationAxis): a shaft across
        // its facing, on the first or second other axis. GaugeBlock too, with no stress
        // (`CStress.setNoImpact`).
        Block::MechanicalPiston
        | Block::StickyMechanicalPiston
        | Block::MechanicalSaw
        | Block::Deployer
        | Block::Speedometer
        | Block::Stressometer => s.facing.map_or([None, None], |f| {
            one(Part::shaft(directional_axis(f, s.along_first)))
        }),
        // CuckooClockBlock (HorizontalKineticBlock): a shaft out of its back
        // (`hasShaftTowards`), on its facing's axis.
        Block::CuckooClock | Block::MysteriousCuckooClock => s
            .facing
            .map_or([None, None], |f| one(Part::half(f.opposite()))),
        // DrillBlock (DirectionalKineticBlock): a shaft out of its back.
        Block::MechanicalDrill => s
            .facing
            .map_or([None, None], |f| one(Part::half(f.opposite()))),
        // PumpBlock: a small cog on its facing's axis, no shaft.
        Block::MechanicalPump => s.facing.map_or([None, None], |f| {
            one(Part {
                shafts: 0,
                ..Part::cog(f.axis(), false)
            })
        }),
        // FluidValveBlock (DirectionalAxisKineticBlock): shafts at both ends of its rotation
        // axis, across its facing and its pipe.
        Block::FluidValve => s.facing.map_or([None, None], |f| {
            one(Part::shaft(directional_axis(f, s.along_first)))
        }),
        // HosePulleyBlock (HorizontalKineticBlock): one shaft, out of the side clockwise of its
        // facing (`hasShaftTowards`); the other side is its pipe's.
        Block::HosePulley => s.facing.map_or([None, None], |f| {
            one(Part::half(crate::wrench::clockwise(f, Axis::Y)))
        }),
        // MechanicalMixerBlock: a small cog on the y axis, no shaft (`hasShaftTowards` false).
        Block::MechanicalMixer => one(Part {
            shafts: 0,
            ..Part::cog(Axis::Y, false)
        }),
        // CrushingWheelBlock (RotatedPillarKineticBlock): shafts at both ends of its axis.
        Block::CrushingWheel => s.axis.map_or([None, None], |a| one(Part::shaft(a))),
        // EncasedFanBlock.hasShaftTowards: the face behind it.
        Block::EncasedFan => s
            .facing
            .map_or([None, None], |f| one(Part::half(f.opposite()))),
        // LargeWaterWheelBlock (RotatedPillarKineticBlock): shafts at both ends of its axis;
        // `LargeWaterWheelBlockEntity.getSize` 2 halves its speed.
        Block::LargeWaterWheel => s.axis.map_or([None, None], |a| {
            one(Part {
                generated: crate::waterwheel::generated_speed(i32::from(settings.flow), 2),
                ..Part::shaft(a)
            })
        }),
        // FlywheelBlock (RotatedPillarKineticBlock): shafts at both ends of its axis, no stress.
        Block::Flywheel => s.axis.map_or([None, None], |a| one(Part::shaft(a))),
        // TurntableBlock (KineticBlock): on the y axis, a shaft only below (`hasShaftTowards`).
        Block::Turntable => one(Part::half(Dir::Down)),
        Block::WaterWheel => s.facing.map_or([None, None], |f| {
            // WaterWheelBlock.hasShaftTowards: both ends of its axis.
            one(Part {
                generated: crate::waterwheel::generated_speed(i32::from(settings.flow), 1),
                ..Part::shaft(f.axis())
            })
        }),
        // ChainConveyorBlock (KineticBlock): on the y axis, shafts up and down
        // (`hasShaftTowards`); its chains join other conveyors (`Kinetics::set_far`).
        Block::ChainConveyor => one(Part::shaft(Axis::Y)),
        // ArmBlock (ICogWheel): a small cog on the y axis, no shaft.
        Block::MechanicalArm => one(Part {
            shafts: 0,
            ..Part::cog(Axis::Y, false)
        }),
        // EjectorBlock (HorizontalKineticBlock): turns on the axis across its facing, shafts at
        // both ends (`getRotationAxis`: the facing turned clockwise).
        Block::WeightedEjector => s.facing.map_or([None, None], |f| {
            let axis = if f.axis() == Axis::X {
                Axis::Z
            } else {
                Axis::X
            };
            one(Part::shaft(axis))
        }),
        _ => [None, None],
    }
}

/// `DirectionalAxisKineticBlock.getRotationAxis`: the first or second axis other than the
/// facing's.
pub fn directional_axis(facing: Dir, along_first: bool) -> Axis {
    match (facing.axis(), along_first) {
        (Axis::X, true) => Axis::Y,
        (Axis::X, false) => Axis::Z,
        (Axis::Y, true) => Axis::X,
        (Axis::Y, false) => Axis::Z,
        (Axis::Z, true) => Axis::X,
        (Axis::Z, false) => Axis::Y,
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
        belt: None,
        powering: false,
        powered_side: false,
        heat: None,
        along_first: false,
        receiver: false,
        inverted: false,
        activate: false,
        tunnel: None,
        chain: None,
        vertical: false,
        sequence: 0,
        face: None,
        target: None,
        level: 0,
        track: None,
        large: false,
        waterlogged: false,
        calling: false,
        ceiling: false,
        extracting: false,
        belt_funnel: None,
        open: false,
        visible: false,
        upper: false,
        hinge_right: false,
        cross_signal: false,
        soul: false,
        enabled: false,
        wall: false,
        size: None,
        extender: None,
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
    fn chains_turn_conveyors_together() {
        // A motor below one chain conveyor; a chain to another 5 blocks east turns it too.
        let mut k = Kinetics::new();
        let motor = info(&[("facing", "up")]);
        let settings = Settings {
            motor_rpm: 32.0,
            ..Settings::default()
        };
        let [m, _] = parts(Block::CreativeMotor, &motor, &settings);
        k.set([0, -1, 0], 0, m.unwrap());
        let none = info(&[]);
        for p in [[0, 0, 0], [5, 0, 0]] {
            k.set(
                p,
                0,
                parts(Block::ChainConveyor, &none, &settings)[0].unwrap(),
            );
        }
        let mut e = Vec::new();
        k.solve(&mut e);
        assert_eq!(k.speed([5, 0, 0], 0), Some(0.0));
        k.set_far([0, 0, 0], vec![[5, 0, 0]]);
        k.set_far([5, 0, 0], vec![[0, 0, 0]]);
        k.solve(&mut e);
        assert_eq!(k.speed([0, 0, 0], 0), Some(32.0));
        assert_eq!(k.speed([5, 0, 0], 0), Some(32.0));
        assert_eq!(Block::ChainConveyor.impact(), 1.0);
        k.set_far([5, 0, 0], vec![]);
        k.solve(&mut e);
        assert_eq!(
            k.speed([5, 0, 0], 0),
            Some(0.0),
            "a chain one side lists only"
        );
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
    fn flywheels_turntables_and_large_wheels_are_create_s_parts() {
        // A motor facing up under a turntable turns it; a flywheel passes rotation on along its
        // axis at no cost; a large wheel's flow gives half a small wheel's speed.
        let mut k = Kinetics::new();
        let motor = info(&[("facing", "up")]);
        k.set(
            [0, -1, 0],
            0,
            parts(Block::CreativeMotor, &motor, &Settings::default())[0].unwrap(),
        );
        let t = parts(Block::Turntable, &info(&[]), &Settings::default())[0].unwrap();
        assert_eq!((t.axis, t.impact), (Axis::Y, 4.0));
        k.set([0, 0, 0], 0, t);
        k.solve(&mut Vec::new());
        assert_eq!(k.speed([0, 0, 0], 0), Some(16.0));
        // Nothing passes out of its top.
        k.set(
            [0, 1, 0],
            0,
            parts(Block::Shaft, &info(&[("axis", "y")]), &Settings::default())[0].unwrap(),
        );
        k.solve(&mut Vec::new());
        assert_eq!(k.speed([0, 1, 0], 0), Some(0.0));
        let f = parts(
            Block::Flywheel,
            &info(&[("axis", "x")]),
            &Settings::default(),
        )[0]
        .unwrap();
        assert_eq!((f.axis, f.impact, f.capacity), (Axis::X, 0.0, 0.0));
        let flowing = Settings {
            flow: -1,
            ..Settings::default()
        };
        let w = parts(Block::LargeWaterWheel, &info(&[("axis", "z")]), &flowing)[0].unwrap();
        assert_eq!((w.axis, w.generated, w.capacity), (Axis::Z, -4.0, 128.0));
        assert!(is_kinetic(Block::LargeWaterWheel) && is_kinetic(Block::Turntable));
    }

    #[test]
    fn a_gauge_passes_rotation_on_and_costs_nothing() {
        let mut k = Kinetics::new();
        let motor = info(&[("facing", "east")]);
        k.set(
            [-1, 0, 0],
            0,
            parts(Block::CreativeMotor, &motor, &Settings::default())[0].unwrap(),
        );
        // Facing up, along the first axis: its shaft runs along x.
        let gauge = info(&[("facing", "up"), ("axis_along_first", "true")]);
        let [g, _] = parts(Block::Stressometer, &gauge, &Settings::default());
        let g = g.unwrap();
        assert_eq!((g.axis, g.impact), (Axis::X, 0.0));
        k.set([0, 0, 0], 0, g);
        let shaft = info(&[("axis", "x")]);
        k.set(
            [1, 0, 0],
            0,
            parts(Block::Shaft, &shaft, &Settings::default())[0].unwrap(),
        );
        k.solve(&mut Vec::new());
        assert_eq!(k.speed([1, 0, 0], 0), Some(16.0));
        let (speed, stress, capacity, over) = k.network([0, 0, 0], 0).unwrap();
        assert_eq!((speed, stress, over), (16.0, 0.0, false));
        assert!(capacity > 0.0);
        assert_eq!(Block::Speedometer.impact(), 0.0);
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
