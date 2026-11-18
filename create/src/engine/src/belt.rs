//! Create's mechanical belt (`BeltBlock`, `BeltBlockEntity`, `BeltHelper`, `BeltInventory`):
//! a chain of segments from a start to an end pulley, turning as one, and the items on it as a
//! plain list on its controller (the start), moved each tick only while the belt turns and holds
//! items.

use crate::kinetics::{Axis, Dir};

/// World operations used by `BeltBlock.onRemove`'s synchronous chain destruction.
/// Replacing a segment must report its block change to the engine before returning.
pub trait BreakWorld {
    /// Take removals discovered by the engine's `BeltBlock.onRemove` translation.
    fn take_breaks(&mut self) -> Vec<crate::world::BeltBreak>;
    /// Write the shaft or air replacement, including the normal block-change notification.
    fn replace(&mut self, broken: crate::world::BeltBreak);
}

/// Complete `BeltBlock.onRemove`: `world.setBlock` runs the next segment's removal
/// before returning, so every descendant is replaced before the next sibling.
/// Both game glue and standalone drivers must use this entry before settling belts.
pub fn break_cascade(world: &mut impl BreakWorld) {
    for broken in world.take_breaks() {
        world.replace(broken);
        break_cascade(world);
    }
}

/// `BeltSlope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slope {
    Horizontal,
    Upward,
    Downward,
    Vertical,
    Sideways,
}

/// `BeltPart`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Start,
    Middle,
    End,
    Pulley,
}

impl Slope {
    /// Its state value.
    pub fn name(self) -> &'static str {
        match self {
            Slope::Horizontal => "horizontal",
            Slope::Upward => "upward",
            Slope::Downward => "downward",
            Slope::Vertical => "vertical",
            Slope::Sideways => "sideways",
        }
    }
}

impl Part {
    /// Its state value.
    pub fn name(self) -> &'static str {
        match self {
            Part::Start => "start",
            Part::Middle => "middle",
            Part::End => "end",
            Part::Pulley => "pulley",
        }
    }
}

/// One belt block's state: `horizontal_facing`, `slope`, `part`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Segment {
    pub facing: Dir,
    pub slope: Slope,
    pub part: Part,
}

impl Segment {
    /// Reads a belt state's properties (None for a state that isn't one).
    pub fn read<'a>(props: impl Iterator<Item = (&'a str, &'a str)>) -> Option<Segment> {
        let (mut facing, mut slope, mut part) = (None, None, None);
        for (k, v) in props {
            match k {
                "facing" => {
                    facing = match v {
                        "north" => Some(Dir::North),
                        "south" => Some(Dir::South),
                        "west" => Some(Dir::West),
                        "east" => Some(Dir::East),
                        _ => None,
                    }
                }
                "slope" => {
                    slope = match v {
                        "horizontal" => Some(Slope::Horizontal),
                        "upward" => Some(Slope::Upward),
                        "downward" => Some(Slope::Downward),
                        "vertical" => Some(Slope::Vertical),
                        "sideways" => Some(Slope::Sideways),
                        _ => None,
                    }
                }
                "part" => {
                    part = match v {
                        "start" => Some(Part::Start),
                        "middle" => Some(Part::Middle),
                        "end" => Some(Part::End),
                        "pulley" => Some(Part::Pulley),
                        _ => None,
                    }
                }
                _ => {}
            }
        }
        Some(Segment {
            facing: facing?,
            slope: slope?,
            part: part?,
        })
    }

    /// `BeltBlock.nextSegmentPosition`: the next segment along the chain (or the one before).
    pub fn next(&self, pos: [i32; 3], forward: bool) -> Option<[i32; 3]> {
        let offset = if forward { 1 } else { -1 };
        if self.part == Part::End && forward || self.part == Part::Start && !forward {
            return None;
        }
        if self.slope == Slope::Vertical {
            let up = if self.facing.positive() {
                offset
            } else {
                -offset
            };
            return Some([pos[0], pos[1] + up, pos[2]]);
        }
        let n = self.facing.offset();
        let mut p = [pos[0] + n[0] * offset, pos[1], pos[2] + n[2] * offset];
        match self.slope {
            Slope::Upward => p[1] += offset,
            Slope::Downward => p[1] -= offset,
            _ => {}
        }
        Some(p)
    }

    /// `BeltHelper.getBeltVector`: from one segment to the next toward the end.
    pub fn vector(&self) -> [i32; 3] {
        if self.slope == Slope::Vertical {
            return [0, if self.facing.positive() { 1 } else { -1 }, 0];
        }
        let n = self.facing.offset();
        let up = match self.slope {
            Slope::Upward => 1,
            Slope::Downward => -1,
            _ => 0,
        };
        [n[0], up, n[2]]
    }

    /// `BeltSlicer.hoveringEnd`: a click at `hit` (from the block's centre) on a start or end
    /// segment is on its outer half.
    pub fn hovering_end(&self, hit: [f64; 3]) -> bool {
        if !matches!(self.part, Part::Start | Part::End) {
            return false;
        }
        let v = self.vector();
        let dot: f64 = (0..3).map(|i| hit[i] * f64::from(v[i])).sum();
        (dot > 0.0) == (self.part == Part::End)
    }

    /// `BeltSlicer.beltStatesCompatible`: a belt `other` may join this one end to end.
    pub fn compatible(&self, other: &Segment) -> bool {
        match self.slope {
            Slope::Upward | Slope::Downward => {
                let flipped = if self.slope == Slope::Upward {
                    Slope::Downward
                } else {
                    Slope::Upward
                };
                if other.slope == flipped {
                    return self.facing == other.facing.opposite();
                }
                other.slope == self.slope && self.facing == other.facing
            }
            _ => other.slope == self.slope && other.facing.axis() == self.facing.axis(),
        }
    }

    /// `BeltSlicer.flipBelt`: the same segment seen from the other end.
    pub fn flipped(&self) -> Segment {
        Segment {
            facing: self.facing.opposite(),
            slope: match self.slope {
                Slope::Upward => Slope::Downward,
                Slope::Downward => Slope::Upward,
                s => s,
            },
            part: match self.part {
                Part::End => Part::Start,
                Part::Start => Part::End,
                p => p,
            },
        }
    }

    /// `BeltBlock.getRotationAxis`.
    pub fn rotation_axis(&self) -> Axis {
        if self.slope == Slope::Sideways {
            return Axis::Y;
        }
        // getClockWise of a horizontal facing: the other horizontal axis.
        match self.facing.axis() {
            Axis::X => Axis::Z,
            _ => Axis::X,
        }
    }

    /// `BeltBlockEntity.hasPulley`: every part but the middle has a shaft through it.
    pub fn has_pulley(&self) -> bool {
        self.part != Part::Middle
    }

    /// `BeltBlock.canTransportObjects`.
    pub fn transports(&self) -> bool {
        !matches!(self.slope, Slope::Vertical | Slope::Sideways)
    }

    fn verticality(&self) -> i32 {
        match self.slope {
            Slope::Downward => -1,
            Slope::Upward => 1,
            _ => 0,
        }
    }
}

/// `BeltBlock.initBelt`'s search for the controller: back along the chain from `pos`, at most
/// 1000 steps. None when the chain is broken (a missing segment) or too long.
pub fn controller_of(
    pos: [i32; 3],
    segment_at: impl Fn([i32; 3]) -> Option<Segment>,
) -> Option<[i32; 3]> {
    let mut current = pos;
    for _ in 0..1000 {
        let s = segment_at(current)?;
        match s.next(current, false) {
            None => return Some(current),
            Some(p) => current = p,
        }
    }
    None
}

/// `BeltBlock.getBeltChain`: the segments from the controller to the end.
pub fn chain(
    controller: [i32; 3],
    segment_at: impl Fn([i32; 3]) -> Option<Segment>,
) -> Vec<[i32; 3]> {
    let mut out = Vec::new();
    let mut current = Some(controller);
    while let Some(p) = current {
        if out.len() >= 1000 {
            break;
        }
        let Some(s) = segment_at(p) else { break };
        out.push(p);
        current = s.next(p, true);
    }
    out
}

/// `BeltHelper.getPositionForOffset` from the controller's segment.
pub fn position_for_offset(
    controller: [i32; 3],
    c: &Segment,
    length: i32,
    offset: i32,
) -> [i32; 3] {
    let n = c.facing.offset();
    [
        controller[0] + offset * n[0],
        controller[1] + offset.clamp(0, (length - 1).max(0)) * c.verticality(),
        controller[2] + offset * n[2],
    ]
}

/// `BeltHelper.getVectorForOffset`: the point on the belt `offset` blocks from its start.
pub fn vector_for_offset(controller: [i32; 3], c: &Segment, length: i32, offset: f32) -> [f64; 3] {
    let mut vertical = c.verticality() as f32;
    if offset < 0.5 {
        vertical = 0.0;
    }
    vertical *= offset.min(length as f32 - 0.5) - 0.5;
    let n = c.facing.offset();
    let horizontal = if c.slope == Slope::Vertical {
        [0.0, 0.0]
    } else {
        [n[0] as f32 * (offset - 0.5), n[2] as f32 * (offset - 0.5)]
    };
    [
        f64::from(controller[0]) + 0.5 + f64::from(horizontal[0]),
        f64::from(controller[1]) + 0.5 + f64::from(vertical),
        f64::from(controller[2]) + 0.5 + f64::from(horizontal[1]),
    ]
}

/// `BeltBlockEntity.getBeltMovementSpeed`: blocks a tick.
pub fn movement_speed(rpm: f32) -> f32 {
    rpm / 480.0
}

/// `getDirectionAwareBeltMovementSpeed`: positive moves items from the start to the end.
pub fn direction_aware_speed(rpm: f32, facing: Dir) -> f32 {
    let mut offset = if facing.positive() { 1.0 } else { -1.0 };
    if facing.axis() == Axis::X {
        offset = -offset;
    }
    movement_speed(rpm) * offset
}

/// `getMovementFacing`: the horizontal way items go.
pub fn movement_facing(rpm: f32, facing: Dir) -> Dir {
    let axis = facing.axis();
    let negative = (movement_speed(rpm) < 0.0) ^ (axis == Axis::X);
    Dir::from_axis(axis, !negative)
}

/// `BeltMovementHandler.transportEntity`: how far an entity standing on the belt segment `seg`
/// at `pos` (feet at `feet`) moves this tick, the belt turning at `rpm`; None when the belt
/// leaves it be (too slow, the entity not on top or off to the side). `steering`: a living
/// entity walking itself, which is not pulled to the middle.
pub fn passenger_motion(
    seg: &Segment,
    pos: [i32; 3],
    feet: [f64; 3],
    rpm: f32,
    steering: bool,
) -> Option<[f64; 3]> {
    if !seg.transports() || rpm.abs() < 1.0 || feet[1] - 0.25 < f64::from(pos[1]) {
        return None;
    }
    let axis = seg.facing.axis();
    let speed = f64::from(movement_speed(rpm));
    // `movementDirection`: negative along x, positive along z.
    let along = if axis == Axis::X { -speed } else { speed };
    let mut m = if axis == Axis::X {
        [along, 0.0, 0.0]
    } else {
        [0.0, 0.0, along]
    };
    let diff_center = if axis == Axis::Z {
        f64::from(pos[0]) + 0.5 - feet[0]
    } else {
        f64::from(pos[2]) + 0.5 - feet[2]
    };
    if diff_center.abs() > 48.0 / 64.0 {
        return None;
    }
    // Up or down a slope (the end's top half and the start's bottom half count as on it).
    let rel = feet[1] - f64::from(pos[1]);
    let top = 13.0 / 16.0;
    let sloped = seg.slope != Slope::Horizontal;
    let (high_end, low_end) = if seg.slope == Slope::Upward {
        (Part::End, Part::Start)
    } else {
        (Part::Start, Part::End)
    };
    let on_slope = sloped
        && (matches!(seg.part, Part::Middle | Part::Pulley)
            || seg.part == high_end && rel < top
            || seg.part == low_end && rel > top);
    // `movementFacing` from the sign of the speed alone.
    let movement_facing = Dir::from_axis(axis, rpm < 0.0);
    let with_facing = movement_facing == seg.facing;
    let mut down = on_slope
        && seg.slope
            == if with_facing {
                Slope::Downward
            } else {
                Slope::Upward
            };
    let mut up = on_slope
        && seg.slope
            == if with_facing {
                Slope::Upward
            } else {
                Slope::Downward
            };
    if axis == Axis::Z {
        std::mem::swap(&mut down, &mut up);
    }
    if up {
        m[1] = along.abs().max(0.13);
    }
    if down {
        m[1] = -along.abs();
    }
    if !steering {
        // `centering`: toward the middle across the belt.
        let pull = diff_center * speed.abs().min(0.1) * 4.0;
        if axis == Axis::Z {
            m[0] += pull;
        } else {
            m[2] += pull;
        }
    }
    Some(m)
}

/// Notes that an item stopped or started moving: clients draw it gliding, so they must hear when
/// that changes (Create resends a belt whose items changed, `notifyUpdate`).
fn set_stalled(item: &mut Item, stalled: bool, out: &mut Ticked) {
    if item.stalled != stalled {
        item.stalled = stalled;
        out.changed = true;
    }
}

/// `BeltBlockEntity.read/write`: persistent controller membership, independent of kinetics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavedSegment {
    pub controller: [i32; 3],
    pub index: i32,
    pub length: i32,
}

/// A stack on a belt (`TransportedItemStack`): how far along it is, sideways offset, angle, the
/// stack, and whether a machine holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub position: f32,
    pub side_offset: f32,
    pub angle: i32,
    pub item: String,
    pub count: u8,
    pub locked: bool,
    pub inserted_at: i32,
    pub inserted_from: Option<Dir>,
    /// `processedBy` / `processingTime`: how far a fan's current has processed it.
    pub fan: crate::fan::Progress,
    /// `lockedExternally`: a mechanical arm reaching for it holds it still for a tick.
    pub held_externally: bool,
    /// A belt funnel facing against the belt holds it at its entry (drawn standing still).
    pub at_funnel: bool,
    /// It didn't move last tick: queued behind another stack or held at the belt's end. Clients
    /// draw it gliding only while this is false, so they are told when it flips. Not saved.
    pub stalled: bool,
    /// The sequenced assembly it is in (its `SequencedAssembly` component): the only component
    /// a stack keeps on a belt.
    pub progress: Option<crate::sequenced::Progress>,
}

impl Item {
    /// TransportedItemStack.copy/getSimilar preserves the transported stack's motion
    /// and processing data, but starts a new stack without transient locks.
    pub fn processing_copy(&self) -> Self {
        let mut copy = self.clone();
        copy.locked = false;
        copy.at_funnel = false;
        copy.stalled = false;
        copy
    }

    /// `new TransportedItemStack(stack)`.
    pub fn new(item: &str, count: u8, angle: i32, upright: bool) -> Item {
        let angle = if upright { 180 } else { angle };
        let mut i = Item {
            position: 0.0,
            side_offset: 0.0,
            angle,
            item: item.to_owned(),
            count,
            locked: false,
            inserted_at: 0,
            inserted_from: None,
            fan: Default::default(),
            held_externally: false,
            at_funnel: false,
            stalled: false,
            progress: None,
        };
        i.side_offset = i.target_side_offset();
        i
    }

    /// `getTargetSideOffset`.
    pub fn target_side_offset(&self) -> f32 {
        (self.angle - 180) as f32 / (360.0 * 3.0)
    }
}

/// Catnip's `AngleHelper.horizontalAngle`: south 0°, west 90°, north 180°, east 270°, the x
/// axis negated.
fn horizontal_angle(d: Dir) -> f32 {
    let y_rot = match d {
        Dir::South => 0.0,
        Dir::West => 90.0,
        Dir::North => 180.0,
        Dir::East => 270.0,
        _ => 0.0,
    };
    if d.axis() == Axis::X { -y_rot } else { y_rot }
}

/// The partial model `BeltRenderer` draws for a segment (`getBeltPartial`), by its partial
/// block's name, top half or `bottom`; None for a diagonal's bottom (it has none).
pub fn partial(seg: &Segment, bottom: bool) -> Option<&'static str> {
    let diagonal = matches!(seg.slope, Slope::Upward | Slope::Downward);
    let (mut start, mut end) = (seg.part == Part::Start, seg.part == Part::End);
    if seg.slope == Slope::Downward || seg.slope == Slope::Vertical && seg.facing.positive() {
        std::mem::swap(&mut start, &mut end);
    }
    Some(match (diagonal, bottom, start, end) {
        (true, true, ..) => return None,
        (true, false, true, _) => "create:part_belt_diagonal_start",
        (true, false, _, true) => "create:part_belt_diagonal_end",
        (true, false, ..) => "create:part_belt_diagonal_middle",
        (false, true, true, _) => "create:part_belt_start_bottom",
        (false, true, _, true) => "create:part_belt_end_bottom",
        (false, true, ..) => "create:part_belt_middle_bottom",
        (false, false, true, _) => "create:part_belt_start",
        (false, false, _, true) => "create:part_belt_end",
        (false, false, ..) => "create:part_belt_middle",
    })
}

/// Every cased segment's model file (`BeltGenerator`).
pub const CASING_MODELS: [&str; 12] = [
    "create:block/belt_casing/diagonal_end",
    "create:block/belt_casing/diagonal_middle",
    "create:block/belt_casing/diagonal_pulley",
    "create:block/belt_casing/diagonal_start",
    "create:block/belt_casing/horizontal_end",
    "create:block/belt_casing/horizontal_middle",
    "create:block/belt_casing/horizontal_pulley",
    "create:block/belt_casing/horizontal_start",
    "create:block/belt_casing/sideways_end",
    "create:block/belt_casing/sideways_middle",
    "create:block/belt_casing/sideways_pulley",
    "create:block/belt_casing/sideways_start",
];

/// The covers over a cased horizontal belt with a block on it (`BeltModel`): andesite or brass,
/// along x or z.
pub const COVER_MODELS: [&str; 4] = [
    "create:block/belt_cover/andesite_belt_cover_x",
    "create:block/belt_cover/andesite_belt_cover_z",
    "create:block/belt_cover/brass_belt_cover_x",
    "create:block/belt_cover/brass_belt_cover_z",
];

/// `BeltGenerator` for a cased segment: its model file and the variant's x and y turns (in
/// degrees, as a block state variant turns its model).
pub fn casing_model(seg: &Segment) -> (&'static str, i32, i32) {
    let downward = seg.slope == Slope::Downward;
    let diagonal = downward || seg.slope == Slope::Upward;
    let vertical = seg.slope == Slope::Vertical;
    let sideways = seg.slope == Slope::Sideways;
    let negative = !seg.facing.positive();
    let mut part = seg.part;
    if (vertical && negative || downward || sideways && negative)
        && !matches!(part, Part::Middle | Part::Pulley)
    {
        part = if part == Part::End {
            Part::Start
        } else {
            Part::End
        };
    }
    let slope = if diagonal {
        "diagonal"
    } else if vertical || sideways {
        "sideways"
    } else {
        "horizontal"
    };
    let name = CASING_MODELS
        .iter()
        .find(|m| m.ends_with(&format!("/{slope}_{}", part.name())))
        .copied()
        .unwrap_or(CASING_MODELS[5]);
    let x = if vertical {
        90
    } else if sideways && negative {
        180
    } else {
        0
    };
    let y_rot = match seg.facing {
        Dir::South => 0,
        Dir::West => 90,
        Dir::North => 180,
        Dir::East => 270,
        _ => 0,
    };
    let y =
        y_rot + if seg.slope == Slope::Upward { 180 } else { 0 } + if vertical { 90 } else { 0 };
    (name, x, y % 360)
}

/// The model file of a belt partial (`getBeltPartial`), by its partial block's name.
pub fn partial_model(partial: &str) -> Option<&'static str> {
    Some(match partial {
        "create:part_belt_diagonal_start" => "create:block/belt/diagonal_start",
        "create:part_belt_diagonal_end" => "create:block/belt/diagonal_end",
        "create:part_belt_diagonal_middle" => "create:block/belt/diagonal_middle",
        "create:part_belt_start_bottom" => "create:block/belt/start_bottom",
        "create:part_belt_end_bottom" => "create:block/belt/end_bottom",
        "create:part_belt_middle_bottom" => "create:block/belt/middle_bottom",
        "create:part_belt_start" => "create:block/belt/start",
        "create:part_belt_end" => "create:block/belt/end",
        "create:part_belt_middle" => "create:block/belt/middle",
        _ => return None,
    })
}

/// Every belt partial's model file.
pub const PARTIAL_MODELS: [&str; 9] = [
    "create:block/belt/diagonal_start",
    "create:block/belt/diagonal_end",
    "create:block/belt/diagonal_middle",
    "create:block/belt/start_bottom",
    "create:block/belt/end_bottom",
    "create:block/belt/middle_bottom",
    "create:block/belt/start",
    "create:block/belt/end",
    "create:block/belt/middle",
];

/// `BeltRenderer.getSpriteShiftEntry`: the sprite a segment's partial is textured with and the
/// scrolling one drawn in its place, for an undyed belt or one dyed `color` (a `DyeColor` name).
pub fn sprite_shift(seg: &Segment, color: Option<&str>, bottom: bool) -> (String, String) {
    let diagonal = matches!(seg.slope, Slope::Upward | Slope::Downward);
    let source = if diagonal {
        "create:block/belt_diagonal"
    } else if bottom {
        "create:block/belt_offset"
    } else {
        "create:block/belt"
    };
    let target = match (color, diagonal) {
        (Some(c), true) => format!("create:block/belt/{c}_diagonal_scroll"),
        (Some(c), false) => format!("create:block/belt/{c}_scroll"),
        (None, true) => "create:block/belt_diagonal_scroll".to_owned(),
        (None, false) => "create:block/belt_scroll".to_owned(),
    };
    (source.to_owned(), target)
}

/// `BeltRenderer`'s UV scroll of a segment's partial turning at `speed` (its rpm) at render
/// time `tick` (client ticks and the partial tick), in units of the scrolling sprite's height
/// (twice the belt sprite's): None when it is drawn with its own sprite (still and undyed).
pub fn scroll(seg: &Segment, speed: f32, dyed: bool, tick: f64, bottom: bool) -> Option<f32> {
    if speed == 0.0 && !dyed {
        return None;
    }
    let downward = seg.slope == Slope::Downward;
    let diagonal = downward || seg.slope == Slope::Upward;
    let sideways = seg.slope == Slope::Sideways;
    let along_x = seg.facing.axis() == Axis::X;
    let step = if seg.facing.positive() { 1.0 } else { -1.0 };
    let time = tick * step;
    let mut speed = f64::from(speed);
    if diagonal && (downward ^ along_x)
        || !sideways && !diagonal && along_x
        || sideways && !seg.facing.positive()
    {
        speed = -speed;
    }
    let mult = if diagonal { 3.0 / 8.0 } else { 0.5 };
    let s = speed * time / (31.5 * 16.0) + if bottom { 0.5 } else { 0.0 };
    Some(((s - s.floor()) * mult) as f32)
}

/// `BeltRenderer`'s turn of a segment's partials, in block units.
pub fn segment_pose(seg: &Segment) -> crate::math::Mat4 {
    use crate::math::{Mat4, Vec3};
    let upward = seg.slope == Slope::Upward;
    let downward = seg.slope == Slope::Downward;
    let sideways = seg.slope == Slope::Sideways;
    let diagonal = upward || downward;
    let y = horizontal_angle(seg.facing)
        + if upward { 180.0 } else { 0.0 }
        + if sideways { 270.0 } else { 0.0 };
    let z: f32 = if sideways { 90.0 } else { 0.0 };
    let x: f32 = if !diagonal && seg.slope != Slope::Horizontal {
        90.0
    } else {
        0.0
    };
    Mat4::from_translation(Vec3::splat(0.5))
        * Mat4::from_rotation_y(y.to_radians())
        * Mat4::from_rotation_z(z.to_radians())
        * Mat4::from_rotation_x(x.to_radians())
        * Mat4::from_translation(Vec3::splat(-0.5))
}

/// `BeltRenderer`'s pulley (`AllPartialModels.BELT_PULLEY`) turned to its shaft's direction
/// (up for a sideways belt, else the facing's clockwise side), in block units.
pub fn pulley_pose(seg: &Segment) -> crate::math::Mat4 {
    use crate::math::{Mat4, Vec3};
    let axis = seg.rotation_axis();
    let mut m = Mat4::from_translation(Vec3::splat(0.5));
    if axis == Axis::X {
        m *= Mat4::from_rotation_y(90f32.to_radians());
    }
    if axis == Axis::Y {
        m *= Mat4::from_rotation_x(90f32.to_radians());
    }
    m * Mat4::from_rotation_x(90f32.to_radians()) * Mat4::from_translation(Vec3::splat(-0.5))
}

/// Where `BeltRenderer.renderItem` puts an item, from the controller block's corner: its point
/// on the belt (`beltStartOffset` + `offsetVec` + side offset), and on a slope the 45° tilt
/// about `tilt_axis` (x, or z for a belt along x).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub at: crate::math::Vec3,
    pub tilt: f32,
    pub tilt_axis: Axis,
}

pub fn item_place(c: &Segment, length: i32, position: f32, side_offset: f32) -> Place {
    use crate::math::Vec3;
    let n = c.facing.offset();
    let dir = Vec3::new(n[0] as f32, n[1] as f32, n[2] as f32);
    let start = -dir * 0.5 + Vec3::new(0.5, 15.0 / 16.0, 0.5);
    let verticality = c.verticality() as f32;
    let vertical = if position < 0.5 {
        0.0
    } else {
        verticality * (position.min(length as f32 - 0.5) - 0.5)
    };
    let mut at = start + dir * position + Vec3::new(0.0, vertical, 0.0);
    // alongX: the side offset runs across the belt.
    let clockwise_x = c.facing.axis() == Axis::Z;
    let side = if clockwise_x {
        side_offset
    } else {
        -side_offset
    };
    at += if clockwise_x {
        Vec3::new(side, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 0.0, side)
    };
    let on_slope =
        c.slope != Slope::Horizontal && position.clamp(0.5, length as f32 - 0.5) == position;
    let tilt_forward =
        ((c.slope == Slope::Downward) ^ c.facing.positive()) == (c.facing.axis() == Axis::Z);
    let tilt: f32 = if !on_slope {
        0.0
    } else if tilt_forward {
        -45.0
    } else {
        45.0
    };
    if on_slope {
        // translate(0, slopeOffset, 0) after the tilt: up the tilted frame.
        at += crate::math::Quat::from_axis_angle(
            if c.facing.axis() == Axis::X {
                Vec3::Z
            } else {
                Vec3::X
            },
            tilt.to_radians(),
        ) * Vec3::new(0.0, 1.0 / 8.0, 0.0);
    }
    Place {
        at,
        tilt,
        tilt_axis: if c.facing.axis() == Axis::X {
            Axis::Z
        } else {
            Axis::X
        },
    }
}

/// `BeltRenderer.renderItem`'s copies of a stack under its point, without the item's own turn:
/// each copy turned 10° (flat items) and lifted, laid flat (flat items) or nudged by
/// `new Random(angle)` (block items), at half size; a package (`PackageItem.isPackage`, a block
/// item) lifted 4 px at one and a half times its size, never nudged.
pub fn item_copies(
    count: u32,
    block_item: bool,
    angle: i32,
    package: bool,
) -> Vec<crate::math::Mat4> {
    use crate::math::{Mat4, Vec3};
    let copies = crate::depot::copies(count);
    let mut rng = crate::depot::JavaRandom::new(i64::from(angle));
    let mut acc = Mat4::IDENTITY;
    let mut out = Vec::new();
    for i in 0..copies {
        let mut m = acc;
        if !block_item {
            m *= Mat4::from_translation(Vec3::new(0.0, -0.09375, 0.0))
                * Mat4::from_rotation_x(90f32.to_radians());
        } else if !package {
            let (dx, dz) = (rng.next_float(), rng.next_float());
            m *= Mat4::from_translation(Vec3::new(
                dx * 0.0625 * i as f32,
                0.0,
                dz * 0.0625 * i as f32,
            ));
        }
        if package {
            m *= Mat4::from_translation(Vec3::new(0.0, 0.25, 0.0));
            out.push(m * Mat4::from_scale(Vec3::splat(1.5)));
        } else {
            out.push(m * Mat4::from_scale(Vec3::splat(0.5)));
        }
        if !block_item {
            acc *= Mat4::from_rotation_y(10f32.to_radians());
        }
        acc *= Mat4::from_translation(Vec3::new(
            0.0,
            if block_item { 1.0 / 64.0 } else { 1.0 / 16.0 },
            0.0,
        ));
    }
    out
}

/// How an item at `position` moves, in blocks a second, on a belt moving `speed` blocks a tick
/// (direction aware): along the belt, and up or down on its slope.
pub fn item_velocity(c: &Segment, length: i32, position: f32, speed: f32) -> crate::math::Vec3 {
    let n = c.facing.offset();
    let per_second = speed * 20.0;
    let vertical = if (0.5..length as f32 - 0.5).contains(&position) {
        c.verticality() as f32
    } else {
        0.0
    };
    crate::math::Vec3::new(n[0] as f32, vertical, n[2] as f32) * per_second
}

/// What lies past the belt's end (`BeltInventory.Ending`), with the margin items stop short of
/// it by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Eject,
    Insert,
    Blocked,
}

impl Ending {
    fn margin(self) -> f32 {
        match self {
            Ending::Eject => 0.0,
            Ending::Insert => 0.25,
            Ending::Blocked => 0.45,
        }
    }
}

/// What a machine over a segment does with an item (`BeltProcessingBehaviour.ProcessingResult`),
/// or what it turned it into (`TransportedResult`: the item is replaced by `held`, put back held
/// at the segment's centre, and `outputs` beside it).
#[derive(Clone, Debug, PartialEq)]
pub enum Process {
    Pass,
    Hold,
    Remove,
    Convert {
        held: Option<Item>,
        outputs: Vec<Item>,
    },
}

/// The belt's surroundings as its inventory asks them.
pub trait Surroundings {
    /// `resolveEnding`.
    fn ending(&mut self) -> Ending;
    /// `DirectBeltInputBehaviour.handleInsertion` into what lies past the end, from the way the
    /// belt moves: what is left of `item` (its count), or None when nothing there takes items.
    fn insert(&mut self, item: &Item) -> Option<u8>;
    /// Whether a machine above segment `segment` would act, and what it does:
    /// `handleReceivedItem` (not `locked`) or `handleHeldItem` (`locked`). None when there is no
    /// machine there, or something blocks it (`isBlocked`, received only).
    fn process(&mut self, segment: i32, item: &mut Item, locked: bool) -> Option<Process>;
    /// `BeltTunnelInteractionHandler.flapTunnelsAndCheckIfStuck`: `item` enters segment
    /// `segment` (-1 past the start); a tunnel over it may take some of it. True when it must
    /// wait where it is.
    fn tunnel(&mut self, segment: i32, item: &mut Item) -> bool {
        let _ = (segment, item);
        false
    }
    /// The belt funnel over segment `segment` (any offset along the belt's line), with the way
    /// the belt moves; None when there is none.
    fn funnel(&mut self, segment: i32) -> Option<(crate::funnel::OnBelt, Dir)> {
        let _ = segment;
        None
    }
    /// The funnel over `segment` inserts `item` into the inventory behind it
    /// (`InvManipulationBehaviour.insert`): the count left, None when it has no inventory or
    /// takes nothing (the client never inserts).
    fn funnel_insert(&mut self, segment: i32, item: &Item) -> Option<u8> {
        let _ = (segment, item);
        None
    }
}

/// What a tick did that the game must act on.
#[derive(Debug, Default, PartialEq)]
pub struct Ticked {
    /// The items changed in a way clients must see (`notifyUpdate`).
    pub changed: bool,
    /// Persisted inventory changed, including transport without a client notification.
    pub saved_changed: bool,
    /// Items thrown off the end (`eject`).
    pub ejected: Vec<Item>,
}

/// `BeltInventory`: the items, furthest along first in the way the belt moves.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inventory {
    pub items: Vec<Item>,
    pub positive: bool,
    to_insert: Vec<Item>,
}

impl Inventory {
    /// An inventory read back from a save.
    pub fn from_saved(items: Vec<Item>, positive: bool) -> Inventory {
        Inventory {
            items,
            positive,
            to_insert: Vec::new(),
        }
    }

    /// What a save keeps: the items and those still waiting to go on (`write` inserts them).
    pub fn to_save(&self) -> Inventory {
        let mut copy = self.clone();
        for i in std::mem::take(&mut copy.to_insert) {
            copy.insert(i);
        }
        copy
    }

    /// Equality of the fields persisted by the belt inventory codec. Queued insertions
    /// participate as in Java's `BeltInventory.write`; client and fan tick caches do not.
    pub fn saved_eq(&self, other: &Self) -> bool {
        if !self.to_insert.is_empty() || !other.to_insert.is_empty() {
            return self.to_save().saved_eq(&other.to_save());
        }
        let (a, b) = (self, other);
        a.positive == b.positive
            && a.items.len() == b.items.len()
            && a.items.iter().zip(&b.items).all(|(a, b)| {
                a.position == b.position
                    && a.side_offset == b.side_offset
                    && a.angle == b.angle
                    && a.item == b.item
                    && a.count == b.count
                    && a.locked == b.locked
                    && a.inserted_at == b.inserted_at
                    && a.inserted_from == b.inserted_from
                    && a.progress == b.progress
            })
    }

    /// Whether it has anything to move or insert.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.to_insert.is_empty()
    }

    /// `addItem`: inserted at the start of the next tick.
    pub fn add(&mut self, item: Item) {
        self.to_insert.push(item);
    }

    fn insert(&mut self, item: Item) {
        // TransportedItemStack.compareTo: further along sorts first.
        let index = self
            .items
            .iter()
            .position(|s| (s.position < item.position) == self.positive)
            .unwrap_or(self.items.len());
        self.items.insert(index, item);
    }

    /// `canInsertAtFromSide`: no item just put on segment `segment` from `side` is still in
    /// the way. `movement` is the belt's movement facing.
    pub fn can_insert_at_from_side(&self, segment: i32, side: Dir, movement: Dir) -> bool {
        let mut at = segment as f32;
        if movement == side.opposite() {
            return false;
        }
        if movement != side {
            at += 0.5;
        } else if !self.positive {
            at += 1.0;
        }
        let blocking = |s: &Item| {
            s.inserted_at == segment
                && s.inserted_from == Some(side)
                && if self.positive {
                    s.position <= at + 1.0
                } else {
                    s.position >= at - 1.0
                }
        };
        !self.items.iter().chain(self.to_insert.iter()).any(blocking)
    }

    /// `applyToEachWithin` taking every item within `max` of `position` off the belt (a player's
    /// empty hand, `BeltBlock.useItemOn`).
    pub fn take_within(&mut self, position: f32, max: f32) -> Vec<Item> {
        let (taken, kept): (Vec<Item>, Vec<Item>) = std::mem::take(&mut self.items)
            .into_iter()
            .partition(|s| (position - s.position).abs() < max);
        self.items = kept;
        taken
    }

    /// `applyToEachWithin`: `f` processes each item within `max` of `position`; one it converts
    /// leaves, its held output put back at the segment's middle and its outputs where it was, all
    /// on the belt from the next tick. True when anything changed.
    pub fn apply_within(
        &mut self,
        position: f32,
        max: f32,
        mut f: impl FnMut(&mut Item) -> Process,
    ) -> bool {
        let mut dirty = false;
        let mut i = 0;
        while i < self.items.len() {
            if (position - self.items[i].position).abs() >= max {
                i += 1;
                continue;
            }
            match f(&mut self.items[i]) {
                Process::Convert { held, outputs } => {
                    dirty = true;
                    self.items.remove(i);
                    if let Some(mut h) = held {
                        let nudge = if self.positive {
                            1.0 / 512.0
                        } else {
                            -1.0 / 512.0
                        };
                        h.position = position.trunc() + 0.5 - nudge;
                        self.to_insert.push(h);
                    }
                    self.to_insert.extend(outputs);
                }
                _ => i += 1,
            }
        }
        dirty
    }

    /// `BeltInventory.tick`'s nonzero-speed direction update. On an empty inventory
    /// this is the whole observable server tick: no surroundings are consulted.
    pub(crate) fn update_direction(&mut self, speed: f32) -> bool {
        if speed == 0.0 || self.positive == (speed > 0.0) {
            return false;
        }
        self.positive = !self.positive;
        self.items.reverse();
        true
    }

    /// `BeltInventory.tick` on the server for a belt of `length` turning at `speed` (direction
    /// aware, blocks a tick); `horizontal` whether machines over it may process.
    pub fn tick(
        &mut self,
        length: i32,
        speed: f32,
        horizontal: bool,
        world: &mut impl Surroundings,
    ) -> Ticked {
        // BeltBlockEntity.write -> BeltInventory.write saves current items even
        // when movement needs no notifyUpdate for clients.
        let before = self.clone();
        let mut out = self.tick_items(length, speed, horizontal, world);
        out.saved_changed = !self.saved_eq(&before);
        out
    }

    fn tick_items(
        &mut self,
        length: i32,
        speed: f32,
        horizontal: bool,
        world: &mut impl Surroundings,
    ) -> Ticked {
        let mut out = Ticked::default();
        if !self.to_insert.is_empty() {
            for item in std::mem::take(&mut self.to_insert) {
                self.insert(item);
            }
            out.changed = true;
        }
        if speed == 0.0 {
            return out;
        }
        out.changed |= self.update_direction(speed);
        let positive = self.positive;
        let spacing = 1.0;
        let mut ending: Option<Ending> = None;
        let mut front: Option<f32> = None;
        // BeltInventory.tick retains currentItem after iterator.remove(); only an item
        // already empty at entry clears it. Keep ownership through every exit path so
        // processing, funnels, insertion and ejection all leave their final position.
        for mut item in std::mem::take(&mut self.items) {
            if item.count == 0 {
                front = None;
                continue;
            }
            let keep = (|| {
                // Held still for a tick by a mechanical arm reaching for it (`lockedExternally`).
                if item.held_externally {
                    item.held_externally = false;
                    return true;
                }
                let mut movement = speed;
                let mut no_movement = false;
                let current = item.position;
                if let Some(f) = front {
                    let diff = f - current;
                    if diff.abs() <= spacing {
                        no_movement = true;
                    }
                    movement = if positive {
                        movement.min(diff - spacing)
                    } else {
                        movement.max(diff + spacing)
                    };
                }
                let mut to_end = if positive {
                    length as f32 - current
                } else {
                    -current
                };
                if to_end.abs() < movement.abs() + 1.0 {
                    let e = *ending.get_or_insert_with(|| world.ending());
                    to_end += if positive { -e.margin() } else { e.margin() };
                }
                let limited = if positive {
                    movement.min(to_end)
                } else {
                    movement.max(to_end)
                };
                let next = current + limited;

                if horizontal {
                    match self.process(&mut item, next, no_movement, world, &mut out) {
                        Step::Removed => return false,
                        Step::Replaced => return false,
                        Step::Held => {
                            let still = current == item.position;
                            set_stalled(&mut item, still, &mut out);
                            return true;
                        }
                        Step::Free => {}
                    }
                }
                // Belt funnels: a funnel over a segment the item reaches takes what it can of it.
                match self.funnels(&mut item, next, length, world, &mut out) {
                    FunnelStep::Gone => return false,
                    FunnelStep::Held => {
                        return true;
                    }
                    FunnelStep::Free => {}
                }
                if no_movement {
                    set_stalled(&mut item, true, &mut out);
                    return true;
                }
                // Belt tunnels, when the item enters another segment.
                let mut upcoming = next as i32;
                if !positive && next == 0.0 {
                    upcoming = -1;
                }
                if current as i32 != upcoming {
                    let before = item.count;
                    let stuck = world.tunnel(upcoming, &mut item);
                    if item.count != before {
                        out.changed = true;
                    }
                    if stuck {
                        set_stalled(&mut item, true, &mut out);
                        return true;
                    }
                }
                // Held at the end the move is a float's rounding away from none: what counts is
                // whether the position changes.
                let still = item.position + limited == item.position;
                set_stalled(&mut item, still, &mut out);
                let item = &mut item;
                item.position += limited;
                let to_middle = item.target_side_offset() - item.side_offset;
                item.side_offset +=
                    (to_middle * limited.abs() * 6.0).clamp(-to_middle.abs(), to_middle.abs());
                if limited == movement {
                    return true;
                }
                // The end is reached.
                match ending.unwrap_or(Ending::Eject) {
                    Ending::Insert => {
                        let Some(left) = world.insert(item) else {
                            return true;
                        };
                        if left == item.count {
                            return true;
                        }
                        out.changed = true;
                        if left == 0 {
                            return false;
                        }
                        item.count = left;
                    }
                    Ending::Blocked => {}
                    Ending::Eject => {
                        out.ejected.push(item.clone());
                        out.changed = true;
                        return false;
                    }
                }
                true
            })();
            front = Some(item.position);
            if keep {
                self.items.push(item);
            }
        }
        out
    }

    /// `handleBeltProcessingAndCheckIfRemoved` for an item about to move to `next`.
    fn process(
        &mut self,
        item: &mut Item,
        next: f32,
        no_movement: bool,
        world: &mut impl Surroundings,
        out: &mut Ticked,
    ) -> Step {
        let positive = self.positive;
        let segment = item.position as i32;
        if item.locked {
            let result = world.process(segment, item, true);
            return match result {
                None => {
                    item.locked = false;
                    out.changed = true;
                    Step::Free
                }
                Some(Process::Hold) => Step::Held,
                Some(Process::Remove) => {
                    out.changed = true;
                    Step::Removed
                }
                Some(Process::Convert { held, outputs }) => {
                    self.replace(segment, held, outputs);
                    out.changed = true;
                    Step::Replaced
                }
                Some(Process::Pass) => {
                    item.locked = false;
                    out.changed = true;
                    Step::Free
                }
            };
        }
        if no_movement {
            return Step::Free;
        }
        if !(item.position > 0.5 || positive) {
            return Step::Free;
        }
        let first = (item.position + if positive { 0.5 } else { -0.5 }) as i32;
        let step = if positive { 1 } else { -1 };
        let mut segment = first;
        while if positive {
            segment as f32 + 0.5 <= next
        } else {
            segment as f32 + 0.5 >= next
        } {
            match world.process(segment, item, false) {
                None | Some(Process::Pass) => {}
                Some(Process::Remove) => {
                    out.changed = true;
                    return Step::Removed;
                }
                Some(Process::Hold) | Some(Process::Convert { .. }) => {
                    item.position =
                        segment as f32 + 0.5 + if positive { 1.0 / 512.0 } else { -1.0 / 512.0 };
                    item.locked = true;
                    out.changed = true;
                    return Step::Held;
                }
            }
            segment += step;
        }
        Step::Free
    }

    /// `BeltFunnelInteractionHandler.checkForFunnels` for an item about to move to `next`: each
    /// funnel it reaches takes what it can into the inventory behind it; one facing against the
    /// belt holds it at its entry.
    fn funnels(
        &mut self,
        item: &mut Item,
        next: f32,
        length: i32,
        world: &mut impl Surroundings,
        out: &mut Ticked,
    ) -> FunnelStep {
        use crate::funnel::Check;
        let positive = self.positive;
        let mut held = false;
        item.at_funnel = false;
        for segment in crate::funnel::segments(item.position, next, positive, length) {
            let Some((f, movement)) = world.funnel(segment) else {
                continue;
            };
            let (blocking, entry) =
                match crate::funnel::check(&f, movement, positive, segment, next) {
                    Check::Skip => continue,
                    Check::Stop => break,
                    Check::Crossed { blocking, entry } => (blocking, entry),
                };
            if blocking && item.position != entry {
                item.position = entry;
                out.changed = true;
            }
            let before = item.count;
            let left = if f.powered {
                None
            } else {
                world.funnel_insert(segment, &*item)
            };
            if let Some(left) = left.filter(|l| *l != before) {
                item.count = left;
                out.changed = true;
                if left == 0 {
                    return FunnelStep::Gone;
                }
            }
            if blocking {
                held = true;
                break;
            }
        }
        if held {
            item.at_funnel = true;
            FunnelStep::Held
        } else {
            FunnelStep::Free
        }
    }

    /// `applyToEachWithin` applying a `TransportedResult`: the item is replaced by what it was
    /// turned into, inserted next tick (`toInsert`).
    fn replace(&mut self, segment: i32, held: Option<Item>, outputs: Vec<Item>) {
        if let Some(mut h) = held {
            h.position = segment as f32 + 0.5
                - if self.positive {
                    1.0 / 512.0
                } else {
                    -1.0 / 512.0
                };
            self.to_insert.push(h);
        }
        self.to_insert.extend(outputs);
    }
}

enum Step {
    Free,
    Held,
    Removed,
    Replaced,
}

enum FunnelStep {
    Free,
    Held,
    Gone,
}

/// The mod-message channel that brings belts' items to clients.
pub const CHANNEL: &str = "create:belt";

/// A belt as clients draw and run it: its controller, controller segment, length (0: gone),
/// what lies past its end, which way its list runs, and its items.
#[derive(Clone, Debug, PartialEq)]
pub struct Contents {
    pub controller: [i32; 3],
    pub segment: Segment,
    pub length: u16,
    pub ending: Ending,
    pub items: Inventory,
}

fn slope_index(s: Slope) -> u8 {
    s as u8
}

fn slope_of(i: u8) -> Slope {
    [
        Slope::Horizontal,
        Slope::Upward,
        Slope::Downward,
        Slope::Vertical,
        Slope::Sideways,
    ][usize::from(i.min(4))]
}

fn part_of(i: u8) -> Part {
    [Part::Start, Part::Middle, Part::End, Part::Pulley][usize::from(i.min(3))]
}

fn dir_of(i: u8) -> Dir {
    Dir::ALL[usize::from(i.min(5))]
}

/// The longest item name the belt and depot messages carry (a package named with its
/// contents is a few thousand bytes).
pub const NAME_MAX: usize = 8192;

fn encode_one(c: &Contents, out: &mut Vec<u8>) {
    for v in c.controller {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&[
        c.segment.facing as u8,
        slope_index(c.segment.slope),
        c.segment.part as u8,
        c.ending as u8,
        u8::from(c.items.positive),
    ]);
    out.extend_from_slice(&c.length.to_le_bytes());
    let items = &c.items.items;
    out.extend_from_slice(&(items.len().min(u16::MAX as usize) as u16).to_le_bytes());
    for i in items.iter().take(u16::MAX as usize) {
        out.extend_from_slice(&i.position.to_le_bytes());
        out.extend_from_slice(&i.side_offset.to_le_bytes());
        out.extend_from_slice(&(i.angle as i16).to_le_bytes());
        // An item named with its components (a package's contents) runs long: two bytes of
        // length; one too long to send whole is left nameless.
        let name = if i.item.len() <= NAME_MAX {
            i.item.as_bytes()
        } else {
            &[]
        };
        // Standing still, by why: bit 0 locked, 1 held at a funnel, 2 stalled.
        let still = u8::from(i.locked) | u8::from(i.at_funnel) << 1 | u8::from(i.stalled) << 2;
        out.extend_from_slice(&[i.count, still]);
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name);
    }
}

/// Messages of at most 30,000 bytes, whole belts in each (a belt longer than that is cut).
pub fn encode(all: &[Contents]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut one = Vec::new();
    for c in all {
        one.clear();
        encode_one(c, &mut one);
        one.truncate(30_000);
        match out.last_mut() {
            Some(m) if m.len() + one.len() <= 30_000 => m.extend_from_slice(&one),
            _ => out.push(one.clone()),
        }
    }
    out
}

pub fn decode(mut d: &[u8]) -> Vec<Contents> {
    fn take<'a>(d: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
        let (a, b) = (d.get(..n)?, d.get(n..)?);
        *d = b;
        Some(a)
    }
    let i32_of = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    let f32_of = |b: &[u8]| f32::from_le_bytes(b.try_into().unwrap());
    let mut out = Vec::new();
    while !d.is_empty() {
        let Some(h) = take(&mut d, 12 + 5 + 2 + 2) else {
            break;
        };
        let mut c = Contents {
            controller: [i32_of(&h[0..4]), i32_of(&h[4..8]), i32_of(&h[8..12])],
            segment: Segment {
                facing: dir_of(h[12]),
                slope: slope_of(h[13]),
                part: part_of(h[14]),
            },
            ending: match h[15] {
                1 => Ending::Insert,
                2 => Ending::Blocked,
                _ => Ending::Eject,
            },
            items: Inventory {
                positive: h[16] != 0,
                ..Inventory::default()
            },
            length: u16::from_le_bytes([h[17], h[18]]),
        };
        for _ in 0..u16::from_le_bytes([h[19], h[20]]) {
            let Some(r) = take(&mut d, 4 + 4 + 2 + 4) else {
                return out;
            };
            let Some(name) = take(&mut d, usize::from(u16::from_le_bytes([r[12], r[13]]))) else {
                return out;
            };
            c.items.items.push(Item {
                position: f32_of(&r[0..4]),
                side_offset: f32_of(&r[4..8]),
                angle: i32::from(i16::from_le_bytes([r[8], r[9]])),
                count: r[10],
                locked: r[11] & 1 != 0,
                item: String::from_utf8_lossy(name).into_owned(),
                inserted_at: 0,
                inserted_from: None,
                fan: Default::default(),
                held_externally: false,
                at_funnel: r[11] & 2 != 0,
                stalled: r[11] & 4 != 0,
                progress: None,
            });
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processing_copy_clears_the_source_stack_lock() {
        let mut item = Item::new("minecraft:bucket", 1, 0, false);
        item.locked = true;
        item.position = 2.5019531;
        let copy = item.processing_copy();
        assert!(!copy.locked);
        assert_eq!(copy.position, item.position);
        assert!(item.locked);
    }

    #[test]
    fn a_belt_carries_what_stands_on_top_of_it_its_way() {
        let seg = Segment {
            facing: Dir::South,
            slope: Slope::Horizontal,
            part: Part::Middle,
        };
        // 48 RPM south: a tenth of a block a tick south, pulled a little toward the middle.
        let m = passenger_motion(&seg, [0, 64, 0], [0.6, 64.8125, 0.5], 48.0, false).unwrap();
        assert!((m[2] - 0.1).abs() < 1e-6 && m[0] < 0.0 && m[1] == 0.0);
        // Nothing for one inside the belt, or on a belt too slow.
        assert!(passenger_motion(&seg, [0, 64, 0], [0.5, 64.1, 0.5], 48.0, false).is_none());
        assert!(passenger_motion(&seg, [0, 64, 0], [0.5, 64.8, 0.5], 0.5, false).is_none());
    }

    #[test]
    fn empty_direction_update_equals_full_inventory_tick() {
        // Java's server tick has no item-loop effects when items/toInsert are empty.
        // Compare every state transition, including stop, restart and reversal.
        for positive in [false, true] {
            let mut full = Inventory::from_saved(Vec::new(), positive);
            let mut idle = full.clone();
            for speed in [0.0, 32.0 / 480.0, 0.0, -32.0 / 480.0, 0.0, 32.0 / 480.0] {
                let ticked = full.tick(5, speed, true, &mut Open(Ending::Eject));
                assert_eq!(idle.update_direction(speed), ticked.changed);
                assert_eq!(idle, full);
                assert!(ticked.ejected.is_empty());
            }
        }
    }

    #[test]
    fn removed_end_stack_blocks_for_the_rest_of_the_tick() {
        // Java issue #27: tick 161 holds pearls at 3.75; tick 162 reaches 3.8166666.
        for horizontal in [false, true] {
            for positive in [false, true] {
                for ending in [Ending::Insert, Ending::Eject] {
                    let end = if ending == Ending::Insert { 4.75 } else { 5.0 };
                    let mut lead = Item::new("minecraft:gold_ingot", 7, 0, false);
                    lead.position = if positive { end } else { 5.0 - end };
                    let mut follower = Item::new("minecraft:ender_pearl", 16, 0, false);
                    follower.position = lead.position + if positive { -1.0 } else { 1.0 };
                    let at = follower.position;
                    let mut inv = Inventory::from_saved(vec![lead, follower], positive);
                    let speed = if positive {
                        32.0 / 480.0
                    } else {
                        -32.0 / 480.0
                    };
                    let mut world = Open(ending);
                    inv.tick(5, speed, horizontal, &mut world);
                    assert_eq!(inv.items.len(), 1);
                    assert_eq!(inv.items[0].position, at);
                    assert!(inv.items[0].stalled);
                    inv.tick(5, speed, horizontal, &mut world);
                    assert_eq!(inv.items[0].position, at + speed);
                    assert!(!inv.items[0].stalled);
                }
            }
        }
    }

    #[test]
    fn empty_stack_clears_the_previous_blocker() {
        let mut lead = Item::new("lead", 1, 0, false);
        lead.position = 2.0;
        lead.held_externally = true;
        let mut empty = Item::new("empty", 0, 0, false);
        empty.position = 1.5;
        let mut follower = Item::new("follower", 1, 0, false);
        follower.position = 1.0;
        let mut inv = Inventory::from_saved(vec![lead, empty, follower], true);
        inv.tick(5, 0.1, true, &mut Open(Ending::Eject));
        assert_eq!(inv.items[1].position, 1.1);
    }

    struct Open(Ending);
    impl Surroundings for Open {
        fn ending(&mut self) -> Ending {
            self.0
        }
        fn insert(&mut self, _: &Item) -> Option<u8> {
            Some(0)
        }
        fn process(&mut self, _: i32, _: &mut Item, _: bool) -> Option<Process> {
            None
        }
    }

    fn seg(facing: Dir, slope: Slope, part: Part) -> Segment {
        Segment {
            facing,
            slope,
            part,
        }
    }

    #[test]
    fn chains_walk_from_the_start_to_the_end() {
        // A belt of three going east and up one: start, middle, end.
        let at = |p: [i32; 3]| match p {
            [0, 0, 0] => Some(seg(Dir::East, Slope::Upward, Part::Start)),
            [1, 1, 0] => Some(seg(Dir::East, Slope::Upward, Part::Middle)),
            [2, 2, 0] => Some(seg(Dir::East, Slope::Upward, Part::End)),
            _ => None,
        };
        assert_eq!(controller_of([2, 2, 0], at), Some([0, 0, 0]));
        assert_eq!(chain([0, 0, 0], at), [[0, 0, 0], [1, 1, 0], [2, 2, 0]]);
        let c = at([0, 0, 0]).unwrap();
        assert_eq!(position_for_offset([0, 0, 0], &c, 3, 2), [2, 2, 0]);
        assert_eq!(
            position_for_offset([0, 0, 0], &c, 3, 3),
            [3, 2, 0],
            "past the end: level"
        );
        assert_eq!(c.rotation_axis(), Axis::Z);
        assert!(c.has_pulley() && !at([1, 1, 0]).unwrap().has_pulley());
    }

    #[test]
    fn items_sit_on_the_belt_as_create_draws_them() {
        let c = seg(Dir::South, Slope::Horizontal, Part::Start);
        // Half way along the first segment of a belt going south: the block's centre, on top.
        let p = item_place(&c, 3, 0.5, 0.0);
        assert!((p.at - crate::math::Vec3::new(0.5, 15.0 / 16.0, 0.5)).length() < 1e-6);
        assert_eq!(p.tilt, 0.0);
        // A belt going up east: past the first half it climbs one per block and tilts.
        let up = seg(Dir::East, Slope::Upward, Part::Start);
        let q = item_place(&up, 3, 1.5, 0.0);
        // The 1/8 lift is in the tilted frame: it also moves it sin 45° / 8 along x.
        assert!((q.at.x - 1.5).abs() - (0.125 * 45f32.to_radians().sin()) < 1e-5);
        assert!(q.at.y > 15.0 / 16.0 + 1.0);
        assert!(q.tilt != 0.0);
        assert_eq!(
            item_velocity(&up, 3, 1.0, 0.1),
            crate::math::Vec3::new(2.0, 2.0, 0.0)
        );
        assert_eq!(
            item_velocity(&up, 3, 0.2, 0.1),
            crate::math::Vec3::new(2.0, 0.0, 0.0)
        );
        assert_eq!(item_copies(16, false, 0, false).len(), 3);
    }

    /// A belt with funnels over some of its segments, each taking up to `room` items.
    struct Funnels {
        at: Vec<(i32, crate::funnel::OnBelt)>,
        room: u8,
        taken: Vec<(i32, u8)>,
    }
    impl Surroundings for Funnels {
        fn ending(&mut self) -> Ending {
            Ending::Blocked
        }
        fn insert(&mut self, _: &Item) -> Option<u8> {
            None
        }
        fn process(&mut self, _: i32, _: &mut Item, _: bool) -> Option<Process> {
            None
        }
        fn funnel(&mut self, segment: i32) -> Option<(crate::funnel::OnBelt, Dir)> {
            let f = self.at.iter().find(|(s, _)| *s == segment)?.1;
            Some((f, Dir::South))
        }
        fn funnel_insert(&mut self, segment: i32, item: &Item) -> Option<u8> {
            let n = item.count.min(self.room);
            self.room -= n;
            self.taken.push((segment, n));
            Some(item.count - n)
        }
    }

    #[test]
    fn funnels_over_a_belt_take_what_passes_them() {
        use crate::funnel::{BeltShape, OnBelt};
        // A belt of four moving south (positive) at a tenth of a block a tick; a funnel facing
        // north over segment 2 (against the belt) with room for 3 of a stack of 5.
        let blocking = OnBelt {
            facing: Dir::North,
            shape: BeltShape::Retracted,
            powered: false,
        };
        let mut world = Funnels {
            at: vec![(2, blocking)],
            room: 3,
            taken: Vec::new(),
        };
        let mut inv = Inventory {
            positive: true,
            ..Default::default()
        };
        let mut item = Item::new("minecraft:iron_ingot", 5, 180, false);
        item.position = 2.0;
        inv.add(item);
        // Once a tick's move would cross the entry at 2.5, 3 go in and the other 2 wait there.
        for _ in 0..6 {
            inv.tick(4, 0.1, true, &mut world);
        }
        assert_eq!(world.taken, [(2, 3)]);
        assert_eq!(inv.items.len(), 1);
        assert_eq!(inv.items[0].count, 2);
        assert_eq!(inv.items[0].position, 2.5);
        // Room again: the rest goes in and the item is gone.
        world.room = 64;
        inv.tick(4, 0.1, true, &mut world);
        assert!(inv.items.is_empty());
        // Powered, a blocking funnel still holds items but takes none.
        world.at[0].1.powered = true;
        let mut item = Item::new("minecraft:iron_ingot", 1, 180, false);
        item.position = 2.45;
        inv.add(item);
        world.taken.clear();
        for _ in 0..3 {
            inv.tick(4, 0.1, true, &mut world);
        }
        assert!(world.taken.is_empty());
        assert_eq!(inv.items[0].position, 2.5);
        // A pulling funnel beside the belt takes from items passing under it, and lets the rest
        // go on.
        let mut world = Funnels {
            at: vec![(
                1,
                OnBelt {
                    facing: Dir::East,
                    shape: BeltShape::Pulling,
                    powered: false,
                },
            )],
            room: 1,
            taken: Vec::new(),
        };
        let mut inv = Inventory {
            positive: true,
            ..Default::default()
        };
        let mut item = Item::new("minecraft:dirt", 2, 180, false);
        item.position = 1.3;
        inv.add(item);
        inv.tick(4, 0.1, true, &mut world);
        inv.tick(4, 0.1, true, &mut world);
        inv.tick(4, 0.1, true, &mut world);
        assert_eq!(world.taken, [(1, 1)]);
        assert_eq!(inv.items[0].count, 1);
        assert!(inv.items[0].position > 1.5);
        // An item an arm reaches for stays still for a tick.
        inv.items[0].held_externally = true;
        let at = inv.items[0].position;
        inv.tick(4, 0.1, true, &mut world);
        assert_eq!(inv.items[0].position, at);
        assert!(!inv.items[0].held_externally);
    }

    #[test]
    fn speeds_follow_the_facing() {
        // getDirectionAwareBeltMovementSpeed: x axis flips the sign.
        assert_eq!(direction_aware_speed(48.0, Dir::South), 0.1);
        assert_eq!(direction_aware_speed(48.0, Dir::East), -0.1);
        assert_eq!(movement_facing(48.0, Dir::East), Dir::West);
        assert_eq!(movement_facing(-48.0, Dir::East), Dir::East);
    }

    #[test]
    fn items_keep_a_block_apart_and_fall_off_the_end() {
        let mut inv = Inventory::default();
        let mut a = Item::new("a", 1, 180, false);
        a.position = 0.5;
        let mut b = Item::new("b", 1, 180, false);
        b.position = 0.0;
        inv.add(a);
        inv.add(b);
        let mut world = Open(Ending::Eject);
        // 96 rpm: 0.2 blocks a tick.
        let speed = movement_speed(96.0);
        inv.tick(3, speed, true, &mut world);
        assert_eq!(inv.items[0].item, "a", "furthest first");
        assert!((inv.items[0].position - 0.7).abs() < 1e-5);
        // Less than a block behind the one ahead, it waits.
        assert_eq!(inv.items[1].position, 0.0);
        let mut ejected = Vec::new();
        for _ in 0..30 {
            ejected.extend(inv.tick(3, speed, true, &mut world).ejected);
        }
        assert_eq!(ejected.len(), 2, "both thrown off the end");
        assert!(inv.items.is_empty());
    }

    #[test]
    fn a_tunnel_takes_part_of_a_stack_as_it_enters_its_segment() {
        // A junction over segment 2 hands one to each of two sides (keeping one on the belt);
        // once its outputs are full the next stack waits before it.
        struct Tunnel {
            entered: Vec<i32>,
            room: u8,
        }
        impl Surroundings for Tunnel {
            fn ending(&mut self) -> Ending {
                Ending::Blocked
            }
            fn insert(&mut self, _: &Item) -> Option<u8> {
                None
            }
            fn process(&mut self, _: i32, _: &mut Item, _: bool) -> Option<Process> {
                None
            }
            fn tunnel(&mut self, segment: i32, item: &mut Item) -> bool {
                self.entered.push(segment);
                if segment != 2 {
                    return false;
                }
                for _ in 0..2 {
                    if item.count <= 1 {
                        break;
                    }
                    if self.room == 0 {
                        return true;
                    }
                    self.room -= 1;
                    item.count -= 1;
                }
                false
            }
        }
        let mut inv = Inventory::default();
        inv.add(Item::new("a", 5, 180, false));
        let mut world = Tunnel {
            entered: vec![],
            room: 2,
        };
        for _ in 0..40 {
            inv.tick(4, movement_speed(96.0), true, &mut world);
        }
        assert_eq!(
            world.entered,
            vec![1, 2, 3],
            "asked once per segment entered"
        );
        assert_eq!(inv.items[0].count, 3);
        let mut inv = Inventory::default();
        inv.add(Item::new("b", 5, 180, false));
        for _ in 0..40 {
            inv.tick(4, movement_speed(96.0), true, &mut world);
        }
        assert!(inv.items[0].position < 2.0, "waits before the full tunnel");
    }

    #[test]
    fn a_blocked_end_stops_items_short() {
        let mut inv = Inventory::default();
        inv.add(Item::new("a", 1, 180, false));
        let mut world = Open(Ending::Blocked);
        for _ in 0..40 {
            inv.tick(2, movement_speed(96.0), true, &mut world);
        }
        assert!((inv.items[0].position - (2.0 - 0.45)).abs() < 1e-5);
    }

    /// A machine over segment 1 holds the item at the segment's centre, then turns it into
    /// something else.
    #[test]
    fn processing_holds_items_at_the_segment_centre() {
        struct Press(u32);
        impl Surroundings for Press {
            fn ending(&mut self) -> Ending {
                Ending::Blocked
            }
            fn insert(&mut self, _: &Item) -> Option<u8> {
                None
            }
            fn process(&mut self, segment: i32, item: &mut Item, locked: bool) -> Option<Process> {
                if segment != 1 {
                    return None;
                }
                self.0 += 1;
                if !locked || self.0 < 5 {
                    return Some(Process::Hold);
                }
                if item.item == "ingot" {
                    let mut sheet = item.clone();
                    sheet.item = "sheet".into();
                    return Some(Process::Convert {
                        held: None,
                        outputs: vec![sheet],
                    });
                }
                Some(Process::Pass)
            }
        }
        let mut inv = Inventory::default();
        inv.add(Item::new("ingot", 1, 180, false));
        let mut press = Press(0);
        // 0.2 a tick: at 1.4 it crosses the segment's centre and is held (tick 8), pressed on
        // the fifth ask (tick 12), the sheet inserted the tick after.
        for _ in 0..13 {
            inv.tick(3, movement_speed(96.0), true, &mut press);
        }
        assert_eq!(inv.items.len(), 1);
        assert_eq!(inv.items[0].item, "sheet");
    }

    #[test]
    fn a_stack_that_stops_at_a_blocked_end_is_sent_once_and_starts_again() {
        let mut belt = Inventory {
            positive: true,
            ..Inventory::default()
        };
        let mut i = Item::new("minecraft:iron_ingot", 8, 0, false);
        i.position = 2.5;
        belt.items.push(i);
        let mut blocked = Open(Ending::Blocked);
        // Rolling toward the end of a 4-long belt: the ticks that move it report nothing new.
        let mut changes = Vec::new();
        for _ in 0..40 {
            changes.push(belt.tick(4, 0.1, true, &mut blocked).changed);
        }
        let item = &belt.items[0];
        assert!(item.stalled, "held at the end");
        assert_eq!(
            changes.iter().filter(|c| **c).count(),
            1,
            "told once, when it stopped: {changes:?}"
        );
        // Turned the other way it moves off again, and that is a change too.
        assert!(belt.tick(4, -0.1, true, &mut blocked).changed);
        assert!(!belt.items[0].stalled);
    }

    #[test]
    fn belts_round_trip_in_messages() {
        let mut items = Inventory {
            positive: true,
            ..Inventory::default()
        };
        let mut i = Item::new("minecraft:iron_ingot", 12, 33, false);
        i.position = 1.25;
        i.locked = true;
        items.items.push(i);
        // Each way of standing still reaches the client on its own.
        for (at_funnel, stalled) in [(true, false), (false, true)] {
            let mut i = Item::new("minecraft:gold_ingot", 3, 0, false);
            (i.at_funnel, i.stalled) = (at_funnel, stalled);
            items.items.push(i);
        }
        let c = Contents {
            controller: [4, 70, -9],
            segment: seg(Dir::West, Slope::Downward, Part::Start),
            length: 5,
            ending: Ending::Insert,
            items,
        };
        let m = encode(std::slice::from_ref(&c));
        assert_eq!(decode(&m[0]), vec![c]);
    }
}

/// Whether an item entity at `position` touches the box `min`..`max` (`ItemEntity`'s box, a
/// quarter block wide and tall from its feet, against `AABB.intersects`).
pub fn item_touches(position: [f64; 3], min: [f64; 3], max: [f64; 3]) -> bool {
    const W: f64 = 0.25;
    let h = W / 2.0;
    let lo = [position[0] - h, position[1], position[2] - h];
    let hi = [position[0] + h, position[1] + W, position[2] + h];
    (0..3).all(|a| lo[a] < max[a] && hi[a] > min[a])
}

#[cfg(test)]
mod item_box_tests {
    #[test]
    fn an_item_touches_a_box_its_quarter_block_reaches() {
        let b = ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        assert!(super::item_touches([0.5, 0.5, 0.5], b.0, b.1));
        // Its box reaches 1/8 sideways and 1/4 up.
        assert!(super::item_touches([1.1, 0.5, 0.5], b.0, b.1));
        assert!(!super::item_touches([1.2, 0.5, 0.5], b.0, b.1));
        assert!(super::item_touches([0.5, -0.2, 0.5], b.0, b.1));
        assert!(!super::item_touches([0.5, 1.0, 0.5], b.0, b.1));
    }
}
