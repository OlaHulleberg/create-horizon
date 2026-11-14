//! What the client draws for Create's turning parts, after its renderers and visuals
//! (`KineticBlockEntityRenderer`, `ShaftVisual`, `GearboxVisual`, `SplitShaftVisual`,
//! `CreativeMotorVisual`): which model each part is, the axis it turns about, its speed and the
//! angle it starts at. And the messages that bring the server's speeds to the client.

use crate::Block;
use crate::kinetics::{Axis, Dir, Part};
use crate::parts::StateInfo;

/// A part's model: the block state's own (a shaft, a cogwheel), Create's full shaft on an axis
/// (inside a casing), or its half shaft pointing out of one face.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Model {
    Own,
    Shaft(Axis),
    ShaftHalf(Dir),
    /// The hand crank's handle (`AllPartialModels.HAND_CRANK_HANDLE`), turned to a face.
    CrankHandle(Dir),
    /// The water wheel (`AllPartialModels.WATER_WHEEL`), its axle along a face.
    WaterWheel(Dir),
    /// The millstone's turning stone (`AllPartialModels.MILLSTONE_COG`).
    MillstoneCog,
    /// The mechanical press's head (`AllPartialModels.MECHANICAL_PRESS_HEAD`), turned to its
    /// facing; it moves down, it does not turn.
    PressHead(Dir),
    /// A belt segment's partial (`BeltRenderer`), its top or bottom half.
    Belt(crate::belt::Segment, bool),
    /// A belt pulley, turning with the belt.
    BeltPulley(crate::belt::Segment),
    /// A windmill bearing's wooden top (`AllPartialModels.BEARING_TOP_WOODEN`), turned to its
    /// facing, turning with its sails.
    BearingTop(Dir),
    /// A mechanical bearing's top (`BEARING_TOP`), the windmill's being wooden.
    BearingTopMetal(Dir),
    /// The encased fan's propeller (`AllPartialModels.ENCASED_FAN_INNER`), turned to a face.
    FanPropeller(Dir),
    /// The mixer's shaftless cog, and its pole and head at rest (`MechanicalMixerRenderer`).
    ShaftlessCog,
    /// The pump's cog (`AllPartialModels.MECHANICAL_PUMP_COG`), turned to its facing.
    PumpCog(Dir),
    /// The drill's head (`AllPartialModels.DRILL_HEAD`), turned to its facing.
    DrillHead(Dir),
    MixerPole,
    MixerHead,
    /// A gauge's head on one face (`AllPartialModels.GAUGE_HEAD_SPEED` / `_STRESS`), and its
    /// dial (`GAUGE_DIAL`), turned to that face; the dial's needle is posed by the glue.
    GaugeHead(Dir, crate::gauge::Kind),
    GaugeDial(Dir),
    /// The mechanical arm's cog (`AllPartialModels.ARM_COG`), on the y axis.
    ArmCog,
    /// The hose pulley's coil (`AllPartialModels.HOSE_COIL`), turned to the positive end of its
    /// shaft's axis (`partialFacing`, as the shaft half).
    HoseCoil(Dir),
}

/// `FlywheelBlockEntity.visualSpeed` on a client: the wheel's drawn speed, which starts at 0 and
/// chases the shaft's (`LerpedFloat.chase(.., 1 / 64f, Chaser.EXP)`, `updateChaseTarget`,
/// `tickChaser`), so a flywheel spins up and down slowly while its shaft changes at once.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelSpin {
    pub visual: f32,
}

impl WheelSpin {
    /// One client tick towards `target` RPM: Catnip's `Chaser.EXP` moves a 64th of the way (in
    /// double, stored as a float), and lands on it once within `Mth.equal`. True while it still
    /// moves: a step too small to change the float leaves it where it is for good, as in Create.
    pub fn tick(&mut self, target: f32) -> bool {
        if (f64::from(target) - f64::from(self.visual)).abs() < 1.0e-5 {
            self.visual = target;
            return false;
        }
        let c = f64::from(self.visual);
        let next = (c + (f64::from(target) - c) / 64.0) as f32;
        let moved = next != self.visual;
        self.visual = next;
        moved
    }
}

/// The starting angle (degrees at clock time 0) that keeps a part turning at `rpm` at the angle
/// it reached at `seconds` turning at `was_rpm` from `was_phase`: a part's angle is
/// `phase + rpm * 6 * seconds`.
pub fn continued_phase(was_phase: f32, was_rpm: f32, rpm: f32, seconds: f32) -> f32 {
    let angle = was_phase + was_rpm * 6.0 * seconds;
    (angle - rpm * 6.0 * seconds).rem_euclid(360.0)
}

/// One part to draw: its slot in its block (part keys are per block and slot), model, axis,
/// speed and starting angle in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Draw {
    pub slot: u8,
    pub model: Model,
    pub axis: Axis,
    pub rpm: f32,
    pub phase: f32,
}

/// `KineticBlockEntityVisual.rotationOffset`: every other block along the axis' plane is turned
/// 22.5° so neighbouring cogs mesh; large cogs otherwise 11.25°.
pub fn rotation_offset(block: Block, axis: Axis, pos: [i32; 3]) -> f32 {
    let sum: i32 = [Axis::X, Axis::Y, Axis::Z]
        .iter()
        .filter(|&&a| a != axis)
        .map(|&a| Axis::of(pos, a))
        .sum();
    if sum % 2 == 0 {
        22.5
    } else if matches!(block, Block::LargeCogwheel) {
        11.25
    } else {
        0.0
    }
}

/// The parts block `block` draws at `pos`, turning at `speeds` (its first and second half).
pub fn draws(block: Block, s: &StateInfo, pos: [i32; 3], speeds: [f32; 2]) -> Vec<Draw> {
    let own = |axis: Axis, model: Model| Draw {
        slot: 0,
        model,
        axis,
        rpm: speeds[0],
        phase: rotation_offset(block, axis, pos),
    };
    let half = |slot: u8, face: Dir, rpm: f32| Draw {
        slot,
        model: Model::ShaftHalf(face),
        axis: face.axis(),
        rpm,
        phase: rotation_offset(block, face.axis(), pos),
    };
    match block {
        Block::Shaft | Block::PoweredShaft | Block::Cogwheel | Block::LargeCogwheel => {
            s.axis.map(|a| vec![own(a, Model::Own)]).unwrap_or_default()
        }
        // KineticBlockEntityRenderer: the gantry shaft's own model turning on its facing's axis.
        Block::GantryShaft => s
            .facing
            .map(|f| vec![own(f.axis(), Model::Own)])
            .unwrap_or_default(),
        // SingleAxisRotatingVisual.shaft: encased shafts, chain drives and the speed controller
        // (its bracket over a cog is not drawn).
        Block::AndesiteEncasedShaft
        | Block::BrassEncasedShaft
        | Block::MetalGirderEncasedShaft
        | Block::EncasedChainDrive
        | Block::AdjustableChainGearshift
        | Block::RotationSpeedController => s
            .axis
            .map(|a| vec![own(a, Model::Shaft(a))])
            .unwrap_or_default(),
        // SplitShaftVisual: a sequenced gearshift's halves, on y when vertical.
        Block::SequencedGearshift => s
            .axis
            .map(|a| {
                let a = if s.vertical { Axis::Y } else { a };
                vec![
                    half(0, Dir::from_axis(a, false), speeds[0]),
                    half(1, Dir::from_axis(a, true), speeds[1]),
                ]
            })
            .unwrap_or_default(),
        Block::Gearbox => s
            .axis
            .map(|a| {
                // Each face's half turns as the gearbox's shaft on that face (its face sign).
                let gearbox = Part::gearbox(a);
                Dir::ALL
                    .into_iter()
                    .filter(|d| d.axis() != a)
                    .enumerate()
                    .map(|(i, d)| {
                        half(i as u8, d, speeds[0] * gearbox.face_sign[d as usize] as f32)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Block::Clutch | Block::Gearshift => s
            .axis
            .map(|a| {
                vec![
                    half(0, Dir::from_axis(a, false), speeds[0]),
                    half(1, Dir::from_axis(a, true), speeds[1]),
                ]
            })
            .unwrap_or_default(),
        // BacktankRenderer: its shaft's half out of the top.
        Block::CopperBacktank | Block::NetheriteBacktank => vec![half(0, Dir::Up, speeds[0])],
        Block::CreativeMotor => s
            .facing
            .map(|f| vec![half(0, f, speeds[0])])
            .unwrap_or_default(),
        // MechanicalPressRenderer: Create's shaft on its axis, and the head (slot 1; its stroke
        // is the press's own, drawn by the glue).
        Block::MechanicalPress => s
            .facing
            .map(|f| {
                vec![
                    own(f.axis(), Model::Shaft(f.axis())),
                    Draw {
                        slot: PRESS_HEAD_SLOT,
                        model: Model::PressHead(f),
                        axis: Axis::Y,
                        rpm: 0.0,
                        phase: 0.0,
                    },
                ]
            })
            .unwrap_or_default(),
        // BeltRenderer: the segment's halves (still: Create scrolls their texture instead), and
        // the pulley turning on the belt's axis.
        Block::Belt => s
            .belt
            .map(|seg| {
                let mut v = vec![Draw {
                    slot: 0,
                    model: Model::Belt(seg, false),
                    axis: Axis::Y,
                    rpm: 0.0,
                    phase: 0.0,
                }];
                if crate::belt::partial(&seg, true).is_some() {
                    v.push(Draw {
                        slot: 1,
                        model: Model::Belt(seg, true),
                        axis: Axis::Y,
                        rpm: 0.0,
                        phase: 0.0,
                    });
                }
                if seg.has_pulley() {
                    let axis = seg.rotation_axis();
                    v.push(Draw {
                        slot: 2,
                        model: Model::BeltPulley(seg),
                        axis,
                        rpm: speeds[0],
                        phase: rotation_offset(block, axis, pos),
                    });
                }
                v
            })
            .unwrap_or_default(),
        // BearingRenderer for a clockwork bearing: the shaft half turning with it, its top
        // still (the hour hand turns it while the clock runs: the glue draws it with the hand).
        Block::ClockworkBearing => s
            .facing
            .map(|f| {
                vec![
                    half(0, f.opposite(), speeds[0]),
                    Draw {
                        slot: 1,
                        model: Model::BearingTopMetal(f),
                        axis: f.axis(),
                        rpm: 0.0,
                        phase: 0.0,
                    },
                ]
            })
            .unwrap_or_default(),
        // AbstractPulleyRenderer, ElevatorPulleyRenderer: a shaft through the pulley.
        Block::RopePulley => s
            .axis
            .map(|a| vec![own(a, Model::Shaft(a))])
            .unwrap_or_default(),
        Block::ElevatorPulley => s
            .facing
            .map(|f| {
                let a = if f.axis() == Axis::X {
                    Axis::Z
                } else {
                    Axis::X
                };
                vec![own(a, Model::Shaft(a))]
            })
            .unwrap_or_default(),
        // BearingRenderer: the shaft half out of its back and its top, both turning with it.
        Block::WindmillBearing | Block::MechanicalBearing => s
            .facing
            .map(|f| {
                vec![
                    half(0, f.opposite(), speeds[0]),
                    Draw {
                        slot: 1,
                        model: if block == Block::WindmillBearing {
                            Model::BearingTop(f)
                        } else {
                            Model::BearingTopMetal(f)
                        },
                        axis: f.axis(),
                        rpm: speeds[0],
                        phase: 0.0,
                    },
                ]
            })
            .unwrap_or_default(),
        // EncasedFanRenderer: the shaft half out of its back, and the propeller (facing back too)
        // turning five times as fast, 80 to 1,280 rpm.
        Block::EncasedFan => s
            .facing
            .map(|f| {
                vec![
                    half(0, f.opposite(), speeds[0]),
                    Draw {
                        slot: 1,
                        model: Model::FanPropeller(f.opposite()),
                        axis: f.axis(),
                        rpm: crate::fan::propeller_rpm(speeds[0]),
                        phase: 0.0,
                    },
                ]
            })
            .unwrap_or_default(),
        // MechanicalMixerRenderer: the cog turns with the mixer, the head at half its speed
        // while idle; the pole doesn't turn.
        Block::MechanicalMixer => vec![
            own(Axis::Y, Model::ShaftlessCog),
            Draw {
                slot: 1,
                model: Model::MixerPole,
                axis: Axis::Y,
                rpm: 0.0,
                phase: 0.0,
            },
            Draw {
                slot: 2,
                model: Model::MixerHead,
                axis: Axis::Y,
                rpm: speeds[0] / 2.0,
                phase: 0.0,
            },
        ],
        // CuckooClockRenderer (a KineticBlockEntityRenderer): the shaft half behind; its hands
        // are the glue's (slots `CUCKOO_HAND_SLOT` on), turned by the day time.
        Block::CuckooClock | Block::MysteriousCuckooClock => s
            .facing
            .map(|f| vec![half(0, f.opposite(), speeds[0])])
            .unwrap_or_default(),
        // DrillRenderer: the shaft half behind, the head turning with it in front.
        Block::MechanicalDrill => s
            .facing
            .map(|f| {
                vec![
                    half(0, f.opposite(), speeds[0]),
                    Draw {
                        slot: 1,
                        model: Model::DrillHead(f),
                        axis: f.axis(),
                        rpm: speeds[0],
                        phase: 0.0,
                    },
                ]
            })
            .unwrap_or_default(),
        // GaugeRenderer (a ShaftRenderer): Create's shaft on its axis, then each shown face's
        // head (slots 1, 2) and dial at rest (slots 3, 4).
        Block::Speedometer | Block::Stressometer => s
            .facing
            .map(|f| {
                let axis = crate::parts::directional_axis(f, s.along_first);
                let kind = if block == Block::Speedometer {
                    crate::gauge::Kind::Speed
                } else {
                    crate::gauge::Kind::Stress
                };
                let mut v = vec![own(axis, Model::Shaft(axis))];
                for (i, face) in crate::gauge::dial_faces(f, axis).enumerate() {
                    let still = |slot: u8, model: Model| Draw {
                        slot,
                        model,
                        axis: Axis::Y,
                        rpm: 0.0,
                        phase: 0.0,
                    };
                    v.push(still(1 + i as u8, Model::GaugeHead(face, kind)));
                    v.push(still(GAUGE_DIAL_SLOT + i as u8, Model::GaugeDial(face)));
                }
                v
            })
            .unwrap_or_default(),
        // FluidValveRenderer (KineticBlockEntityRenderer of a shaft on its axis): the shaft
        // turns; the pointer is the glue's (`VALVE_POINTER_SLOT`), it follows the valve's own
        // pointer.
        Block::FluidValve => s
            .facing
            .map(|f| {
                let a = crate::parts::directional_axis(f, s.along_first);
                vec![own(a, Model::Shaft(a))]
            })
            .unwrap_or_default(),
        // HosePulleyRenderer (AbstractPulleyRenderer): the shaft through it on its axis
        // (`getShaftAxis`: the facing turned clockwise) and the coil, still (Create scrolls its
        // texture instead).
        Block::HosePulley => s
            .facing
            .map(|f| {
                let a = crate::wrench::clockwise(f, Axis::Y).axis();
                vec![
                    own(a, Model::Shaft(a)),
                    Draw {
                        slot: 1,
                        model: Model::HoseCoil(Dir::from_axis(a, true)),
                        axis: Axis::Y,
                        rpm: 0.0,
                        phase: 0.0,
                    },
                ]
            })
            .unwrap_or_default(),
        // PumpRenderer: its cog turns with it.
        Block::MechanicalPump => s
            .facing
            .map(|f| vec![own(f.axis(), Model::PumpCog(f))])
            .unwrap_or_default(),
        // CrushingWheelBlock: its whole model turns on its axis (`ENTITYBLOCK_ANIMATED`).
        Block::CrushingWheel => s.axis.map(|a| vec![own(a, Model::Own)]).unwrap_or_default(),
        // MillstoneRenderer: the inner stone turns on the y axis.
        Block::Millstone => vec![own(Axis::Y, Model::MillstoneCog)],
        // ArmRenderer.getRotatedModel: its cog turns on the y axis (the arm is drawn apart).
        Block::MechanicalArm => vec![own(Axis::Y, Model::ArmCog)],
        // EjectorRenderer (ShaftRenderer): a shaft across its facing.
        Block::WeightedEjector => s
            .facing
            .map(|f| {
                let a = if f.axis() == Axis::X {
                    Axis::Z
                } else {
                    Axis::X
                };
                vec![own(a, Model::Shaft(a))]
            })
            .unwrap_or_default(),
        // WaterWheelVisual: the wheel, turning on its axis.
        // WaterWheelRenderer (large): the block's own model turning on its axis.
        // KineticBlockEntityRenderer: the turntable's own model on y.
        Block::LargeWaterWheel => s.axis.map(|a| vec![own(a, Model::Own)]).unwrap_or_default(),
        Block::Turntable => vec![own(Axis::Y, Model::Own)],
        // FlywheelRenderer: Create's shaft on its axis, and the wheel (its own model) turning
        // from angle 0 (`FlywheelBlockEntity.angle`).
        Block::Flywheel => s
            .axis
            .map(|a| {
                vec![
                    own(a, Model::Shaft(a)),
                    Draw {
                        slot: 1,
                        model: Model::Own,
                        axis: a,
                        rpm: speeds[0],
                        phase: 0.0,
                    },
                ]
            })
            .unwrap_or_default(),
        Block::WaterWheel => s
            .facing
            .map(|f| vec![own(f.axis(), Model::WaterWheel(f))])
            .unwrap_or_default(),
        // HandCrankVisual: its shaft half into the block it hangs from, and the handle facing
        // away from it, both turning with the crank.
        Block::HandCrank => s
            .facing
            .map(|f| {
                vec![
                    half(0, f.opposite(), speeds[0]),
                    Draw {
                        slot: 1,
                        model: Model::CrankHandle(f.opposite()),
                        axis: f.axis(),
                        rpm: speeds[0],
                        phase: 0.0,
                    },
                ]
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The channel the server sends speeds on, and the client asks for them on.
pub const CHANNEL: &str = "create:kinetics";
const SET: u8 = 1;
const CLEAR: u8 = 2;
/// Bytes of one record (kind, position, half, speed), and records per message (mod messages
/// carry at most 30,000 bytes).
const RECORD: usize = 1 + 12 + 1 + 4;
pub const PER_MESSAGE: usize = 30_000 / RECORD;

/// A change the client applies: a part's speed (0 when stopped or overstressed), or a block
/// whose parts are gone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Update {
    Set { pos: [i32; 3], sub: u8, speed: f32 },
    Clear { pos: [i32; 3] },
}

/// Updates as messages of at most 30,000 bytes each.
pub fn encode(updates: &[Update]) -> Vec<Vec<u8>> {
    updates
        .chunks(PER_MESSAGE)
        .map(|chunk| {
            let mut out = Vec::with_capacity(chunk.len() * RECORD);
            for u in chunk {
                let (kind, pos, sub, speed) = match *u {
                    Update::Set { pos, sub, speed } => (SET, pos, sub, speed),
                    Update::Clear { pos } => (CLEAR, pos, 0, 0.0),
                };
                out.push(kind);
                for c in pos {
                    out.extend_from_slice(&c.to_le_bytes());
                }
                out.push(sub);
                out.extend_from_slice(&speed.to_le_bytes());
            }
            out
        })
        .collect()
}

/// A message's updates; a malformed tail (a server of another version) is dropped.
pub fn decode(data: &[u8]) -> Vec<Update> {
    data.chunks_exact(RECORD)
        .filter_map(|r| {
            let int = |i: usize| i32::from_le_bytes(r[i..i + 4].try_into().unwrap());
            let pos = [int(1), int(5), int(9)];
            match r[0] {
                SET => Some(Update::Set {
                    pos,
                    sub: r[13],
                    speed: f32::from_le_bytes(r[14..18].try_into().unwrap()),
                }),
                CLEAR => Some(Update::Clear { pos }),
                _ => None,
            }
        })
        .collect()
}

/// A part's key: its block and slot, packed (x and z 26 bits each, y 9 bits from -64, slot 3).
pub fn key(pos: [i32; 3], slot: u8) -> u64 {
    let x = (pos[0] as u64) & 0x3FF_FFFF;
    let z = (pos[2] as u64) & 0x3FF_FFFF;
    let y = ((pos[1] + 64) as u64) & 0x1FF;
    x | z << 26 | y << 52 | u64::from(slot & 7) << 61
}

/// The press head's slot among its block's parts.
pub const PRESS_HEAD_SLOT: u8 = 1;

/// A gauge's first dial slot (its second dial is the next).
pub const GAUGE_DIAL_SLOT: u8 = 3;

/// A fluid valve's pointer's slot.
pub const VALVE_POINTER_SLOT: u8 = 1;

/// A cuckoo clock's hour hand's slot (its minute hand is the next).
pub const CUCKOO_HAND_SLOT: u8 = 1;
/// The cuckoo clock's doors (left, right) and its figure, the glue's too.
pub const CUCKOO_DOOR_SLOT: u8 = 3;
pub const CUCKOO_FIGURE_SLOT: u8 = 5;

/// Slots a block may use, to clear them all.
pub const SLOTS: u8 = 8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_and_split_at_the_size_limit() {
        let updates: Vec<Update> = (0..PER_MESSAGE as i32 + 3)
            .map(|i| Update::Set {
                pos: [i, -64, -i],
                sub: (i % 2) as u8,
                speed: i as f32 * -0.5,
            })
            .chain([Update::Clear { pos: [1, 2, 3] }])
            .collect();
        let messages = encode(&updates);
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|m| m.len() <= 30_000));
        let back: Vec<Update> = messages.iter().flat_map(|m| decode(m)).collect();
        assert_eq!(back, updates);
    }

    #[test]
    fn keys_are_distinct_per_block_and_slot() {
        let a = key([-30_000_000, -64, 30_000_000], 0);
        assert_ne!(a, key([-30_000_000, -64, 30_000_000], 1));
        assert_ne!(a, key([-30_000_000, -63, 30_000_000], 0));
        assert_ne!(key([1, 0, 0], 0), key([0, 0, 1], 0));
    }

    #[test]
    fn offsets_alternate_as_create_s_visuals() {
        // Axis Z: x + y even turns 22.5°.
        assert_eq!(rotation_offset(Block::Cogwheel, Axis::Z, [0, 0, 5]), 22.5);
        assert_eq!(rotation_offset(Block::Cogwheel, Axis::Z, [1, 0, 5]), 0.0);
        assert_eq!(
            rotation_offset(Block::LargeCogwheel, Axis::Z, [1, 0, 5]),
            11.25
        );
        assert_eq!(rotation_offset(Block::Cogwheel, Axis::Z, [-1, 0, 5]), 0.0);
    }

    #[test]
    fn a_hand_crank_shows_its_half_shaft_and_handle() {
        let s = StateInfo {
            facing: Some(Dir::West),
            ..StateInfo::default()
        };
        let d = draws(Block::HandCrank, &s, [0, 0, 0], [-32.0, 0.0]);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].model, Model::ShaftHalf(Dir::East));
        assert_eq!(d[1].model, Model::CrankHandle(Dir::East));
        assert!(d.iter().all(|x| x.axis == Axis::X && x.rpm == -32.0));
    }

    #[test]
    fn a_gauge_shows_its_shaft_and_a_head_and_dial_on_each_side() {
        let s = StateInfo {
            facing: Some(Dir::Up),
            along_first: true,
            ..StateInfo::default()
        };
        let d = draws(Block::Speedometer, &s, [0, 0, 0], [16.0, 0.0]);
        assert_eq!(d.len(), 5);
        assert_eq!((d[0].model, d[0].rpm), (Model::Shaft(Axis::X), 16.0));
        assert_eq!(
            d[1].model,
            Model::GaugeHead(Dir::North, crate::gauge::Kind::Speed)
        );
        assert_eq!((d[2].slot, d[2].model), (3, Model::GaugeDial(Dir::North)));
        assert_eq!((d[4].slot, d[4].model), (4, Model::GaugeDial(Dir::South)));
    }

    #[test]
    fn a_gearbox_shows_four_halves_turning_with_its_faces() {
        let s = StateInfo {
            axis: Some(Axis::Y),
            ..StateInfo::default()
        };
        let d = draws(Block::Gearbox, &s, [0, 0, 0], [8.0, 0.0]);
        assert_eq!(d.len(), 4);
        let east = d
            .iter()
            .find(|x| x.model == Model::ShaftHalf(Dir::East))
            .unwrap();
        let west = d
            .iter()
            .find(|x| x.model == Model::ShaftHalf(Dir::West))
            .unwrap();
        assert_eq!((east.rpm, west.rpm), (8.0, -8.0));
        assert_eq!(east.axis, Axis::X);
    }
}

#[cfg(test)]
mod wheel_tests {
    use super::{WheelSpin, continued_phase};

    #[test]
    fn a_flywheel_spins_up_a_64th_at_a_time_and_lands() {
        let mut w = WheelSpin::default();
        assert!(w.tick(64.0));
        assert_eq!(w.visual, 1.0);
        assert!(w.tick(64.0));
        assert_eq!(w.visual, 1.0 + 63.0 / 64.0);
        // Half way after ln(2) / -ln(63/64) = 44 ticks.
        for _ in 2..44 {
            w.tick(64.0);
        }
        assert!((w.visual - 32.0).abs() < 0.5, "{}", w.visual);
        let mut ticks = 44;
        while w.tick(64.0) {
            ticks += 1;
        }
        // It stops where a 64th of the gap no longer moves the float: 63.999878, as Create's.
        assert_eq!(w.visual, 63.999878);
        assert!(ticks > 600, "{ticks}");
        // Stopped: it winds down the same way.
        let at = w.visual;
        assert!(w.tick(0.0));
        assert_eq!(w.visual, at - at / 64.0);
    }

    #[test]
    fn a_new_speed_continues_the_angle() {
        // 10 RPM from 30 degrees: 30 + 60 * 2.5 = 180 at 2.5 s; at 20 RPM from there.
        let phase = continued_phase(30.0, 10.0, 20.0, 2.5);
        assert!(((phase + 20.0 * 6.0 * 2.5).rem_euclid(360.0) - 180.0).abs() < 1e-3);
    }
}
