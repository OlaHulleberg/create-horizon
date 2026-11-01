//! Create's mechanical belt (`BeltBlock`, `BeltBlockEntity`, `BeltHelper`, `BeltInventory`):
//! a chain of segments from a start to an end pulley, turning as one, and the items on it as a
//! plain list on its controller (the start), moved each tick only while the belt turns and holds
//! items.

use crate::kinetics::{Axis, Dir};

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
}

impl Item {
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

/// `BeltRenderer`'s turn of a segment's partials, in block units.
pub fn segment_pose(seg: &Segment) -> glam::Mat4 {
    use glam::{Mat4, Vec3};
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
pub fn pulley_pose(seg: &Segment) -> glam::Mat4 {
    use glam::{Mat4, Vec3};
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
    pub at: glam::Vec3,
    pub tilt: f32,
    pub tilt_axis: Axis,
}

pub fn item_place(c: &Segment, length: i32, position: f32, side_offset: f32) -> Place {
    use glam::Vec3;
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
        at += glam::Quat::from_axis_angle(
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
/// `new Random(angle)` (block items), at half size.
pub fn item_copies(count: u32, block_item: bool, angle: i32) -> Vec<glam::Mat4> {
    use glam::{Mat4, Vec3};
    let copies = crate::depot::copies(count);
    let mut rng = crate::depot::JavaRandom::new(i64::from(angle));
    let mut acc = Mat4::IDENTITY;
    let mut out = Vec::new();
    for i in 0..copies {
        let mut m = acc;
        if !block_item {
            m *= Mat4::from_translation(Vec3::new(0.0, -0.09375, 0.0))
                * Mat4::from_rotation_x(90f32.to_radians());
        } else {
            let (dx, dz) = (rng.next_float(), rng.next_float());
            m *= Mat4::from_translation(Vec3::new(
                dx * 0.0625 * i as f32,
                0.0,
                dz * 0.0625 * i as f32,
            ));
        }
        out.push(m * Mat4::from_scale(Vec3::splat(0.5)));
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
pub fn item_velocity(c: &Segment, length: i32, position: f32, speed: f32) -> glam::Vec3 {
    let n = c.facing.offset();
    let per_second = speed * 20.0;
    let vertical = if (0.5..length as f32 - 0.5).contains(&position) {
        c.verticality() as f32
    } else {
        0.0
    };
    glam::Vec3::new(n[0] as f32, vertical, n[2] as f32) * per_second
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
}

/// What a tick did that the game must act on.
#[derive(Debug, Default, PartialEq)]
pub struct Ticked {
    /// The items changed in a way clients must see (`notifyUpdate`).
    pub changed: bool,
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

    /// `BeltInventory.tick` on the server for a belt of `length` turning at `speed` (direction
    /// aware, blocks a tick); `horizontal` whether machines over it may process.
    pub fn tick(
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
        if self.positive != (speed > 0.0) {
            self.positive = !self.positive;
            self.items.reverse();
            out.changed = true;
        }
        let positive = self.positive;
        let spacing = 1.0;
        let mut ending: Option<Ending> = None;
        let mut front: Option<f32> = None;
        let mut i = 0;
        while i < self.items.len() {
            if self.items[i].count == 0 {
                self.items.remove(i);
                continue;
            }
            let mut movement = speed;
            let mut no_movement = false;
            let current = self.items[i].position;
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
                match self.process(i, next, no_movement, world, &mut out) {
                    Step::Removed => continue,
                    Step::Replaced => continue,
                    Step::Held => {
                        front = Some(self.items[i].position);
                        i += 1;
                        continue;
                    }
                    Step::Free => {}
                }
            }
            if no_movement {
                front = Some(self.items[i].position);
                i += 1;
                continue;
            }
            let item = &mut self.items[i];
            item.position += limited;
            let to_middle = item.target_side_offset() - item.side_offset;
            item.side_offset +=
                (to_middle * limited.abs() * 6.0).clamp(-to_middle.abs(), to_middle.abs());
            if limited == movement {
                front = Some(item.position);
                i += 1;
                continue;
            }
            // The end is reached.
            match ending.unwrap_or(Ending::Eject) {
                Ending::Insert => {
                    let Some(left) = world.insert(&self.items[i]) else {
                        front = Some(self.items[i].position);
                        i += 1;
                        continue;
                    };
                    if left == self.items[i].count {
                        front = Some(self.items[i].position);
                        i += 1;
                        continue;
                    }
                    out.changed = true;
                    if left == 0 {
                        self.items.remove(i);
                        continue;
                    }
                    self.items[i].count = left;
                }
                Ending::Blocked => {}
                Ending::Eject => {
                    out.ejected.push(self.items.remove(i));
                    out.changed = true;
                    continue;
                }
            }
            front = Some(self.items[i].position);
            i += 1;
        }
        out
    }

    /// `handleBeltProcessingAndCheckIfRemoved` for item `i` about to move to `next`.
    fn process(
        &mut self,
        i: usize,
        next: f32,
        no_movement: bool,
        world: &mut impl Surroundings,
        out: &mut Ticked,
    ) -> Step {
        let positive = self.positive;
        let segment = self.items[i].position as i32;
        if self.items[i].locked {
            let result = world.process(segment, &mut self.items[i], true);
            return match result {
                None => {
                    self.items[i].locked = false;
                    out.changed = true;
                    Step::Free
                }
                Some(Process::Hold) => Step::Held,
                Some(Process::Remove) => {
                    self.items.remove(i);
                    out.changed = true;
                    Step::Removed
                }
                Some(Process::Convert { held, outputs }) => {
                    self.replace(i, segment, held, outputs);
                    out.changed = true;
                    Step::Replaced
                }
                Some(Process::Pass) => {
                    self.items[i].locked = false;
                    out.changed = true;
                    Step::Free
                }
            };
        }
        if no_movement {
            return Step::Free;
        }
        let item = &self.items[i];
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
            match world.process(segment, &mut self.items[i], false) {
                None | Some(Process::Pass) => {}
                Some(Process::Remove) => {
                    self.items.remove(i);
                    out.changed = true;
                    return Step::Removed;
                }
                Some(Process::Hold) | Some(Process::Convert { .. }) => {
                    let item = &mut self.items[i];
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

    /// `applyToEachWithin` applying a `TransportedResult`: the item is replaced by what it was
    /// turned into, inserted next tick (`toInsert`).
    fn replace(&mut self, i: usize, segment: i32, held: Option<Item>, outputs: Vec<Item>) {
        self.items.remove(i);
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
        let name = &i.item.as_bytes()[..i.item.len().min(255)];
        out.extend_from_slice(&[i.count, u8::from(i.locked), name.len() as u8]);
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
            let Some(r) = take(&mut d, 4 + 4 + 2 + 3) else {
                return out;
            };
            let Some(name) = take(&mut d, usize::from(r[12])) else {
                return out;
            };
            c.items.items.push(Item {
                position: f32_of(&r[0..4]),
                side_offset: f32_of(&r[4..8]),
                angle: i32::from(i16::from_le_bytes([r[8], r[9]])),
                count: r[10],
                locked: r[11] != 0,
                item: String::from_utf8_lossy(name).into_owned(),
                inserted_at: 0,
                inserted_from: None,
                fan: Default::default(),
            });
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!((p.at - glam::Vec3::new(0.5, 15.0 / 16.0, 0.5)).length() < 1e-6);
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
            glam::Vec3::new(2.0, 2.0, 0.0)
        );
        assert_eq!(
            item_velocity(&up, 3, 0.2, 0.1),
            glam::Vec3::new(2.0, 0.0, 0.0)
        );
        assert_eq!(item_copies(16, false, 0).len(), 3);
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
    fn belts_round_trip_in_messages() {
        let mut items = Inventory {
            positive: true,
            ..Inventory::default()
        };
        let mut i = Item::new("minecraft:iron_ingot", 12, 33, false);
        i.position = 1.25;
        i.locked = true;
        items.items.push(i);
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
