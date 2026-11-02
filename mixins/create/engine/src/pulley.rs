//! Create's rope pulley (`PulleyBlockEntity`, `PulleyContraption`, `LinearActuatorBlockEntity`):
//! the structure under its rope moves down and up at the pulley's speed, `speed / 512` blocks a
//! tick, and goes back into the world at a whole block where it stops, with rope and a magnet
//! between it and the pulley.

use crate::contraption::{Found, Refused, gather_from};
use std::collections::HashSet;

/// `kinetics.maxRopeLength`.
pub const MAX_ROPE_LENGTH: i32 = 384;

/// `PulleyBlockEntity.getExtensionRange`: the rope reaches no lower than the build limit.
pub fn extension_range(y: i32, min_y: i32) -> i32 {
    MAX_ROPE_LENGTH.min((y - 1) - min_y).max(0)
}

/// `getMovementSpeed` on the server (`convertToLinear`: `speed / 512`, held below half a block):
/// positive extends (down).
pub fn movement(rpm: f32) -> f32 {
    (rpm / 512.0).clamp(-0.49, 0.49)
}

/// `getGridOffset`: `(int) (offset + .5)` held in the range.
pub fn grid_offset(offset: f32, range: i32) -> i32 {
    ((offset + 0.5) as i32).clamp(0, range)
}

/// `PulleyContraption.assemble` from `anchor` (the block under the rope): what holds on to it,
/// never the rope, magnet and pulley above it (`isAnchoringBlockAt`). Offsets from the anchor.
pub fn gather(
    anchor: [i32; 3],
    initial: i32,
    at: impl Fn([i32; 3]) -> Option<Found>,
) -> Result<Vec<([i32; 3], u32)>, Refused> {
    let rope = |p: [i32; 3]| {
        p[0] == anchor[0]
            && p[2] == anchor[2]
            && p[1] > anchor[1]
            && p[1] <= anchor[1] + initial + 1
    };
    Ok(gather_from(anchor, anchor, rope, at)?.0)
}

/// `getOrCreateColliders`: the blocks of a structure with no block of it next to them in a
/// direction (`up` true: above), the ones that meet the world first moving that way.
pub fn colliders(blocks: &[([i32; 3], u32)], up: bool) -> Vec<[i32; 3]> {
    let set: HashSet<[i32; 3]> = blocks.iter().map(|(p, _)| *p).collect();
    let dy = if up { 1 } else { -1 };
    blocks
        .iter()
        .map(|(p, _)| *p)
        .filter(|p| !set.contains(&[p[0], p[1] + dy, p[2]]))
        .collect()
}

/// A running pulley (`running`, `offset`) and the structure it moves (`movedContraption`).
#[derive(Clone, Debug, PartialEq)]
pub struct Pulley {
    /// `contraption.anchor`: where its top block was when it started, `initialOffset` below
    /// the pulley's rope.
    pub anchor: [i32; 3],
    pub initial: i32,
    pub offset: f32,
    pub range: i32,
    /// The speed it last ran at.
    pub rpm: f32,
    /// Offsets from the anchor, and states; none for a rope with nothing on it.
    pub blocks: Vec<([i32; 3], u32)>,
    /// Its blocks with nothing of it below and above them.
    pub down: Vec<[i32; 3]>,
    pub up: Vec<[i32; 3]>,
}

/// What a tick of a running pulley does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// It moved this many blocks (positive down).
    Move(f32),
    /// It stops at this whole offset (`disassemble`): at an end, or against the world.
    Stop(i32),
    /// An empty rope reached a block: it stops here and starts again with it next tick
    /// (`visitNewPosition`: `disassemble`, `assembleNextTick`).
    Grab(i32),
}

impl Pulley {
    pub fn new(anchor: [i32; 3], initial: i32, range: i32, blocks: Vec<([i32; 3], u32)>) -> Self {
        Pulley {
            anchor,
            initial,
            offset: initial as f32,
            range,
            rpm: 0.0,
            down: colliders(&blocks, false),
            up: colliders(&blocks, true),
            blocks,
        }
    }

    /// `LinearActuatorBlockEntity.onSpeedChanged` while it moves: turned the other way, it
    /// first drops to the whole offset below (`Math.round(offset * 16) / 16`, an integer
    /// division in Create).
    pub fn speed_changed(&mut self, rpm: f32) {
        if self.rpm != 0.0 && self.rpm.signum() != rpm.signum() && !self.blocks.is_empty() {
            self.offset = ((self.offset * 16.0).round() as i32 / 16) as f32;
        }
        self.rpm = rpm;
    }

    /// `toPosition`: the world y of its anchor's row at `offset`.
    pub fn base(&self, offset: f32) -> f32 {
        (self.anchor[1] + self.initial) as f32 - offset
    }

    /// Where its anchor is when it stops at whole offset `grid`.
    pub fn placed_at(&self, grid: i32) -> [i32; 3] {
        [
            self.anchor[0],
            self.anchor[1] + self.initial - grid,
            self.anchor[2],
        ]
    }

    /// Whether the structure at `offset` overlaps a world cell that isn't free
    /// (`ContraptionCollider.collideBlocks`), its leading blocks the way it moves.
    pub fn collides(&self, offset: f32, down: bool, free: &impl Fn([i32; 3]) -> bool) -> bool {
        let base = self.base(offset);
        let leading = if down { &self.down } else { &self.up };
        leading.iter().any(|l| {
            let y0 = base + l[1] as f32;
            let cell = if down {
                y0.floor() as i32
            } else {
                (y0 + 1.0).ceil() as i32 - 1
            };
            !free([self.anchor[0] + l[0], cell, self.anchor[2] + l[2]])
        })
    }

    /// One tick (`LinearActuatorBlockEntity.tick` while running). `free` says whether a world
    /// cell lets the structure through; `grab` whether an empty rope's magnet takes the block
    /// at a cell (`isMovementNecessary`, not brittle). `pulley` is the pulley's position.
    pub fn step(
        &mut self,
        rpm: f32,
        pulley: [i32; 3],
        free: impl Fn([i32; 3]) -> bool,
        grab: impl Fn([i32; 3]) -> bool,
    ) -> Step {
        let speed = movement(rpm);
        let new = self.offset + speed;
        // `visitNewPosition`: an empty rope going down stops at a block that needs moving.
        if new as i32 != self.offset as i32 && self.blocks.is_empty() && rpm > 0.0 {
            let below = [pulley[0], pulley[1] - (new as i32 + 1), pulley[2]];
            if grab(below) {
                return Step::Grab(grid_offset(self.offset, self.range));
            }
        }
        // `moveAndCollideContraption`, then `collided`: back to the whole offset it came from.
        if !self.blocks.is_empty() && speed != 0.0 && self.collides(new, speed > 0.0, &free) {
            let at = grid_offset(self.offset, self.range) as f32;
            return Step::Stop(grid_offset(at - speed, self.range));
        }
        self.offset = new;
        if self.offset <= 0.0 || self.offset >= self.range as f32 {
            let end = if self.offset <= 0.0 {
                0.0
            } else {
                self.range as f32
            };
            return Step::Stop(grid_offset(end, self.range));
        }
        Step::Move(speed)
    }
}

/// Where `PulleyBlockEntity.disassemble` puts rope (`ropes`, below the pulley) and the magnet
/// for a stop at whole offset `grid`: rope at 1 to `grid - 1` below, the magnet at `grid`.
pub fn rope_cells(pulley: [i32; 3], grid: i32) -> (Vec<[i32; 3]>, Option<[i32; 3]>) {
    let at = |i: i32| [pulley[0], pulley[1] - i, pulley[2]];
    let ropes = (1..grid).map(at).collect();
    ((ropes), (grid > 0).then(|| at(grid)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stone(state: u32) -> Option<Found> {
        Some(Found {
            state,
            sail_facing: None,
            sticky: None,
            movable: true,
            chassis: None,
            ..Default::default()
        })
    }

    #[test]
    fn its_range_and_speed_are_create_s() {
        assert_eq!(extension_range(100, -64), 163);
        assert_eq!(extension_range(400, -64), 384);
        assert_eq!(extension_range(-64, -64), 0);
        assert_eq!(movement(64.0), 0.125);
        assert_eq!(movement(-256.0), -0.49);
        assert_eq!(grid_offset(2.49, 10), 2);
        assert_eq!(grid_offset(2.5, 10), 3);
        assert_eq!(grid_offset(12.0, 10), 10);
    }

    #[test]
    fn it_takes_what_hangs_under_its_rope_but_not_the_rope() {
        // A pulley at y 10 with two rope blocks and a magnet (y 7..9): the anchor at y 6 holds
        // a slime block, a stone beside it is stuck to it; a rope cell would be skipped.
        let at = |p: [i32; 3]| match p {
            [0, 6, 0] => Some(Found {
                sticky: Some(crate::contraption::Sticky::Slime),
                ..stone(1).unwrap()
            }),
            [1, 6, 0] | [0, 7, 0] => stone(2),
            _ => None,
        };
        let got = gather([0, 6, 0], 3, at).unwrap();
        assert_eq!(got, vec![([0, 0, 0], 1), ([1, 0, 0], 2)]);
        assert_eq!(colliders(&got, false), vec![[0, 0, 0], [1, 0, 0]]);
    }

    #[test]
    fn it_extends_until_the_floor_and_stops_at_a_whole_block() {
        // A pulley at y 10, a stone hanging at y 9 (no rope yet), the floor at y 4: it falls
        // 0.125 a tick at 64 rpm and stops resting on the floor, its anchor at y 5.
        let pulley = [0, 10, 0];
        let mut p = Pulley::new([0, 9, 0], 0, extension_range(10, -64), vec![([0, 0, 0], 1)]);
        let free = |c: [i32; 3]| c[1] > 4;
        let mut ticks = 0;
        let stop = loop {
            ticks += 1;
            match p.step(64.0, pulley, free, |_| false) {
                Step::Move(v) => assert_eq!(v, 0.125),
                Step::Stop(at) => break at,
                Step::Grab(_) => unreachable!(),
            }
        };
        assert_eq!(stop, 4);
        assert_eq!(p.placed_at(stop), [0, 5, 0]);
        assert_eq!(ticks, 33, "4 blocks at 1/8 a tick, then the collision");
        let (ropes, magnet) = rope_cells(pulley, stop);
        assert_eq!(ropes, vec![[0, 9, 0], [0, 8, 0], [0, 7, 0]]);
        assert_eq!(magnet, Some([0, 6, 0]));
    }

    #[test]
    fn it_stops_at_its_length_and_back_at_the_top() {
        // A pulley at y -60 reaches 3 blocks (the build limit at -64); nothing below.
        let pulley = [0, -60, 0];
        let range = extension_range(-60, -64);
        assert_eq!(range, 3);
        let mut p = Pulley::new([0, -61, 0], 0, range, vec![([0, 0, 0], 1)]);
        let stop = loop {
            if let Step::Stop(at) = p.step(256.0, pulley, |_| true, |_| false) {
                break at;
            }
        };
        assert_eq!(stop, 3);
        // Going up from offset 2: back to 0, the structure under the pulley.
        let mut up = Pulley::new([0, -63, 0], 2, range, vec![([0, 0, 0], 1)]);
        let stop = loop {
            if let Step::Stop(at) = up.step(-128.0, pulley, |_| true, |_| false) {
                break at;
            }
        };
        assert_eq!((stop, up.placed_at(stop)), (0, [0, -61, 0]));
    }

    #[test]
    fn an_empty_rope_grabs_the_block_it_reaches() {
        // Empty rope from a pulley at y 10, a stone at y 7: at offset 2 it stops to take it.
        let pulley = [0, 10, 0];
        let mut p = Pulley::new([0, 9, 0], 0, 20, Vec::new());
        let grab = |c: [i32; 3]| c[1] == 7;
        let at = loop {
            match p.step(64.0, pulley, |_| true, grab) {
                Step::Grab(at) => break at,
                Step::Stop(_) => unreachable!(),
                Step::Move(_) => {}
            }
        };
        assert_eq!(at, 2);
    }
}
