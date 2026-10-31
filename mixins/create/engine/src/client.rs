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
        Block::AndesiteEncasedShaft | Block::BrassEncasedShaft | Block::MetalGirderEncasedShaft => {
            s.axis
                .map(|a| vec![own(a, Model::Shaft(a))])
                .unwrap_or_default()
        }
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
        // MillstoneRenderer: the inner stone turns on the y axis.
        Block::Millstone => vec![own(Axis::Y, Model::MillstoneCog)],
        // WaterWheelVisual: the wheel, turning on its axis.
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
