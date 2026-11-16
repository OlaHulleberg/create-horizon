//! Create's elevator (`ElevatorPulleyBlockEntity`, `ElevatorContraption`, `ElevatorColumn`,
//! `ElevatorContactBlock`): a structure hanging from an elevator pulley with one redstone
//! contact on its side; the elevator contacts in the column that contact faces are its floors,
//! and a contact given a redstone signal calls it there. It speeds up and slows down on its
//! own, at most twice a rope pulley's speed, and settles level with the floor.

use crate::kinetics::Dir;

/// An elevator column (`ColumnCoords`): the x and z the contraption's contact faces into, and
/// the way the floors' contacts face (toward the elevator).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ColumnKey {
    pub x: i32,
    pub z: i32,
    pub side: Dir,
}

/// `ElevatorColumn`: its contacts' heights (sorted), the floor it is called to and whether an
/// elevator runs in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Column {
    pub contacts: Vec<i32>,
    /// `targetedYLevel` once `targetAvailable`.
    pub target: Option<i32>,
    pub active: bool,
}

impl Column {
    /// `add`: kept sorted, once each.
    pub fn add(&mut self, y: i32) {
        if let Err(i) = self.contacts.binary_search(&y) {
            self.contacts.insert(i, y);
        }
    }

    pub fn remove(&mut self, y: i32) {
        self.contacts.retain(|c| *c != y);
    }

    /// `checkEmpty`: a column with no contacts and no elevator is forgotten.
    pub fn is_empty(&self) -> bool {
        self.contacts.is_empty() && !self.active
    }
}

/// `ElevatorContactBlock.getColumnCoords`: a contact's column is its own x and z and facing.
pub fn column_of(pos: [i32; 3], facing: Dir) -> ColumnKey {
    ColumnKey {
        x: pos[0],
        z: pos[2],
        side: facing,
    }
}

/// `ElevatorContraption.capture` of the contraption's redstone contact (facing sideways) at
/// `local` (from the anchor): the column it faces into (the floors face back at it) and its
/// height in the structure (`contactYOffset`).
pub fn contraption_column(anchor: [i32; 3], local: [i32; 3], facing: Dir) -> (ColumnKey, i32) {
    let o = facing.offset();
    let front = [local[0] + o[0], local[1] + o[1], local[2] + o[2]];
    (
        ColumnKey {
            x: anchor[0] + front[0],
            z: anchor[2] + front[2],
            side: facing.opposite(),
        },
        front[1],
    )
}

/// `RedstoneContactBlock.hasValidContact`: the block a contact faces is a contact facing back.
pub fn faces_back(facing: Dir, other: Option<Dir>) -> bool {
    other == Some(facing.opposite())
}

/// `ElevatorContactBlock.getSignal`: 15 while powering, none into another elevator contact.
pub fn contact_signal(powering: bool, reader_is_contact: bool) -> i32 {
    if powering && !reader_is_contact {
        15
    } else {
        0
    }
}

/// `RedstoneContactBlock.getSignal` toward `side` (from the reader to the contact): 15 while
/// powered, none back into the contact it faces.
pub fn redstone_contact_signal(powered: bool, facing: Dir, side: Dir) -> i32 {
    if powered && side != facing.opposite() {
        15
    } else {
        0
    }
}

/// `getMovementSpeed`: toward `target` from `offset`, no faster than `speed × 2 / 512`
/// (at most 1.99 a tick), changing by at most `lerp(that, 0.0075, 0.0175)` a tick, and slowing
/// down to stop there (`sqrt(2 × distance × acceleration)`). Whether it has arrived (within
/// half a block).
pub fn movement(target: i32, offset: f32, rpm: f32, prev: f32) -> (f32, bool) {
    let diff = target as f32 - offset;
    let movement = (rpm * 2.0 / 512.0).clamp(-1.99, 1.99);
    let limit = movement.abs();
    let acceleration = 0.0075 + limit * (0.0175 - 0.0075);
    let deceleration = (2.0 * diff.abs() * acceleration).sqrt();
    let speed = diff
        .clamp(-limit, limit)
        .clamp(prev - acceleration, prev + acceleration)
        .clamp(-deceleration, deceleration);
    (speed, diff.abs() < 0.5)
}

/// A running elevator: where it was put together, its offset below the pulley (`offset`), its
/// structure's height in the world (the entity's y, apart from the offset while it settles),
/// its last speed, whether it has arrived, and its contact's column.
#[derive(Clone, Debug, PartialEq)]
pub struct Elevator {
    pub anchor: [i32; 3],
    pub initial: i32,
    pub offset: f32,
    pub y: f64,
    pub prev: f32,
    pub arrived: bool,
    pub range: i32,
    pub column: ColumnKey,
    /// `contactYOffset`, `minContactY`, `maxContactY`.
    pub contact_y: i32,
    pub min_contact: i32,
    pub max_contact: i32,
    pub blocks: Vec<([i32; 3], u32)>,
}

/// What a tick did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tick {
    /// It arrived: the contact at this height in its column lights up (`triggerContact`).
    pub reached: Option<i32>,
    /// Its height changed by this much.
    pub moved: f64,
}

impl Elevator {
    /// `ElevatorPulleyBlockEntity.assemble`'s bookkeeping: anchored `initial` blocks under the
    /// pulley at `pulley_y`, its contact `contact_y` up in it.
    pub fn new(
        pulley_y: i32,
        anchor: [i32; 3],
        initial: i32,
        range: i32,
        column: ColumnKey,
        contact_y: i32,
        blocks: Vec<([i32; 3], u32)>,
    ) -> Elevator {
        let max_contact = pulley_y + contact_y - 1;
        Elevator {
            anchor,
            initial,
            offset: initial as f32,
            y: anchor[1] as f64,
            prev: 0.0,
            arrived: true,
            range,
            column,
            contact_y,
            min_contact: max_contact - crate::pulley::MAX_ROPE_LENGTH,
            max_contact,
            blocks,
        }
    }

    /// The floor (contact height) the column calls it to that it can reach
    /// (`getCurrentTargetY`).
    pub fn reachable(&self, target: Option<i32>) -> Option<i32> {
        target.filter(|y| (self.min_contact..=self.max_contact).contains(y))
    }

    /// `assemble`'s first target: the floor level with its contact now.
    pub fn floor_now(&self, pulley_y: i32) -> i32 {
        pulley_y + self.contact_y - 1 - self.offset as i32
    }

    /// `toPosition`.
    pub fn base(&self, offset: f32) -> f64 {
        f64::from(self.anchor[1] + self.initial) - f64::from(offset)
    }

    /// One call of `getMovementSpeed`: it keeps the speed (`prevSpeed`) and whether it arrived.
    fn speed(&mut self, target_offset: i32, rpm: f32) -> f32 {
        let (s, arrived) = movement(target_offset, self.offset, rpm, self.prev);
        self.prev = s;
        self.arrived = arrived;
        s
    }

    /// One tick (`LinearActuatorBlockEntity.tick`, then `ElevatorPulleyBlockEntity.tick`),
    /// called to `target` (the column's floor, if it can reach it). `getMovementSpeed` runs as
    /// often as Create calls it: once, or three times while it travels.
    pub fn tick(&mut self, pulley_y: i32, rpm: f32, target: Option<i32>) -> Tick {
        let was_arrived = self.arrived;
        let before = self.y;
        let target = self.reachable(target);
        let target_offset =
            target.map_or(self.offset as i32, |t| pulley_y - t + self.contact_y - 1);
        let first = self.speed(target_offset, rpm);
        let new = self.offset + first;
        // `moveAndCollideContraption`: no collisions for an elevator, and no move once there.
        if !self.arrived {
            let s = self.speed(target_offset, rpm);
            self.speed(target_offset, rpm);
            self.y -= f64::from(s);
        }
        self.offset = new;
        if self.offset <= 0.0 || self.offset >= self.range as f32 {
            self.offset = self.offset.clamp(0.0, self.range as f32);
            if !self.arrived {
                self.speed(target_offset, rpm);
                self.speed(target_offset, rpm);
            }
            self.y = self.base(self.offset);
        }
        let mut reached = None;
        if self.arrived {
            let level = target.unwrap_or((0.5 + self.y).floor() as i32 + self.contact_y);
            if !was_arrived {
                reached = Some(level);
            }
            let mut diff = f64::from(level) - self.y - f64::from(self.contact_y);
            if diff.abs() > 1.0 / 128.0 {
                diff *= 0.25;
            }
            self.y += diff;
        }
        Tick {
            reached,
            moved: self.y - before,
        }
    }

    /// Nothing more to do until it is called elsewhere: there, level with its floor, still.
    pub fn settled(&self, target: Option<i32>) -> bool {
        let Some(level) = self.reachable(target) else {
            return self.arrived && self.prev.abs() < 1.0 / 1024.0;
        };
        self.arrived
            && self.prev.abs() < 1.0 / 1024.0
            && (self.y + f64::from(self.contact_y) - f64::from(level)).abs() < 1e-6
    }

    /// Where it goes back into the world (`disassemble`: at the whole offset nearest).
    pub fn placed_at(&self) -> [i32; 3] {
        let grid = crate::pulley::grid_offset(self.offset, self.range);
        [
            self.anchor[0],
            self.anchor[1] + self.initial - grid,
            self.anchor[2],
        ]
    }
}

/// One floor of an elevator's column as its contraption controls list it
/// (`ElevatorColumn.compileNamesList`): its contact's height and names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Floor {
    pub y: i32,
    pub short: String,
    pub long: String,
}

/// What an elevator's contraption controls show (`ElevatorFloorSelection` after
/// `tickFloorSelection`): the floor picked, its names, and whether it is the floor the elevator
/// is called to. `unreachable`: the long name is `contraption.controls.floor_unreachable`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub target_y: i32,
    pub short: String,
    pub long: String,
    pub unreachable: bool,
    pub matches_target: bool,
}

/// `ContraptionControlsMovement.tickFloorSelection`: the picked `index` kept within the floors
/// (`X`, `No Floors` without any), read against the floor the elevator is called to
/// (`clientYTarget`) and the contacts it can reach (`isTargetUnreachable`).
pub fn select_floor(
    index: &mut i32,
    floors: &[Floor],
    client_target: i32,
    min_contact: i32,
    max_contact: i32,
) -> Selection {
    if floors.is_empty() {
        *index = 0;
        return Selection {
            target_y: 0,
            short: "X".into(),
            long: "No Floors".into(),
            unreachable: false,
            matches_target: true,
        };
    }
    *index = (*index).clamp(0, floors.len() as i32 - 1);
    let f = &floors[*index as usize];
    Selection {
        target_y: f.y,
        short: f.short.clone(),
        long: f.long.clone(),
        unreachable: f.y < min_contact || f.y > max_contact,
        matches_target: f.y == client_target,
    }
}

/// `syncControlDisplays`: the index of the floor the elevator is called to, which every one of
/// its controls shows when the floors or the call change.
pub fn floor_index(floors: &[Floor], client_target: i32) -> Option<i32> {
    floors
        .iter()
        .rposition(|f| f.y == client_target)
        .map(|i| i as i32)
}

/// Catnip's `LerpedFloat`, as contraption controls' animations use it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lerped {
    pub previous: f32,
    pub value: f32,
    pub target: f32,
    pub speed: f32,
    /// The chaser: none (`null`), `LINEAR` or `EXP`.
    pub chaser: Option<bool>,
    pub angular: bool,
}

/// `AngleHelper.getShortestAngleDiff`.
fn shortest_angle_diff(current: f64, target: f64) -> f32 {
    let (c, t) = (current % 360.0, target % 360.0);
    (((((t - c) % 360.0) + 540.0) % 360.0) - 180.0) as f32
}

/// `Mth.equal`.
fn equal(a: f64, b: f64) -> bool {
    (b - a).abs() < 1.0e-5
}

impl Lerped {
    /// `setValue`.
    pub fn set(&mut self, v: f32) {
        self.previous = self.value;
        self.value = v;
    }

    /// `updateChaseTarget`: an angular one takes the shortest way.
    pub fn update_target(&mut self, target: f32) {
        self.target = if self.angular {
            self.value + shortest_angle_diff(f64::from(self.value), f64::from(target))
        } else {
            target
        };
    }

    /// `chase(value, speed, LINEAR or EXP)`.
    pub fn chase(&mut self, target: f32, speed: f32, linear: bool) {
        self.update_target(target);
        self.speed = speed;
        self.chaser = Some(linear);
    }

    /// `tickChaser`.
    pub fn tick(&mut self) {
        self.previous = self.value;
        let Some(linear) = self.chaser else {
            return;
        };
        if equal(f64::from(self.value), f64::from(self.target)) {
            self.value = self.target;
            return;
        }
        let (c, s, t) = (
            f64::from(self.value),
            f64::from(self.speed),
            f64::from(self.target),
        );
        self.value = if linear {
            (c + (t - c).clamp(-s, s)) as f32
        } else {
            (c + (t - c) * s) as f32
        };
    }

    /// `getValue(partialTicks)`: linear, or `AngleHelper.angleLerp` for an angular one.
    pub fn at(&self, partial: f32) -> f32 {
        let (p, c, t) = (
            f64::from(partial),
            f64::from(self.previous),
            f64::from(self.value),
        );
        if self.angular {
            (c + f64::from(shortest_angle_diff(c, t)) * p) as f32
        } else {
            (c + (t - c) * p) as f32
        }
    }
}

/// A contraption controls block's animations on a moving contraption as its client runs them
/// (`ContraptionControlsBlockEntity.button` and `indicator`), and on an elevator the floor it
/// shows (`ElevatorFloorSelection.currentIndex`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlsAnim {
    pub button: Lerped,
    pub indicator: Lerped,
    pub index: i32,
}

impl Default for ControlsAnim {
    fn default() -> Self {
        ControlsAnim {
            // `LerpedFloat.linear().startWithValue(0).chase(0, 0.125f, Chaser.EXP)`.
            button: Lerped {
                speed: 0.125,
                chaser: Some(false),
                ..Lerped::default()
            },
            // `LerpedFloat.angular().startWithValue(0)`.
            indicator: Lerped {
                angular: true,
                ..Lerped::default()
            },
            index: 0,
        }
    }
}

impl ControlsAnim {
    /// `pressButton`.
    pub fn press(&mut self) {
        self.button.set(1.0);
    }

    /// `tickAnimations`.
    pub fn tick(&mut self) {
        self.button.tick();
        self.indicator.tick();
    }

    /// `ContraptionControlsMovement.tick` on an elevator after `tickFloorSelection`: the
    /// animations, then the indicator turning in eighths toward the floor the elevator is called
    /// to (`clientYTarget`) from the height its contact is at (`currentY`), settling at 0 there.
    pub fn tick_elevator(&mut self, client_target: i32, current_y: i32) {
        self.tick();
        let at_target = client_target == current_y;
        let indicator = &mut self.indicator;
        let current = indicator.target;
        let below = if at_target {
            current > 0.0
        } else {
            client_target <= current_y
        };
        if current == 0.0 && !at_target {
            let start = if below { 181.0 } else { -181.0 };
            indicator.set(start);
            indicator.update_target(start);
            self.tick();
            return;
        }
        let stage = (((current % 360.0) + 360.0) % 360.0).floor() as i32;
        if !at_target || stage / 45 != 0 {
            let increment = if stage / 45 == if below { 4 } else { 3 } {
                2.25
            } else {
                33.75
            };
            let step = if below { increment } else { -increment };
            indicator.chase(current + step, 45.0, true);
            return;
        }
        indicator.set(0.0);
        indicator.update_target(0.0);
    }

    /// `ContraptionControlsRenderer`: the indicator's partial (`CONTRAPTION_CONTROLS_INDICATOR`
    /// 0 to 7) this frame.
    pub fn indicator_part(&self, partial: f32) -> usize {
        let v = self.indicator.at(partial) as i32;
        (((v / 45) % 8 + 8) % 8) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elevator() -> Elevator {
        // A pulley at y 20; the car's top under 4 free blocks (anchor y 15), its contact one
        // block down in it facing east (contact y offset -1, column at x 1).
        let (column, contact_y) = contraption_column([0, 15, 0], [0, -1, 0], Dir::East);
        assert_eq!(
            column,
            ColumnKey {
                x: 1,
                z: 0,
                side: Dir::West
            }
        );
        assert_eq!(contact_y, -1);
        Elevator::new(
            20,
            [0, 15, 0],
            4,
            crate::pulley::extension_range(20, -64),
            column,
            contact_y,
            vec![([0, 0, 0], 1), ([0, -1, 0], 2)],
        )
    }

    #[test]
    fn it_accelerates_and_slows_down_to_its_floor() {
        // 64 rpm: 0.25 blocks a tick at most, 0.01 a tick more each time.
        let (s, arrived) = movement(10, 0.0, 64.0, 0.0);
        assert!((s - 0.01).abs() < 1e-6 && !arrived, "{s}");
        let (s, _) = movement(10, 0.0, 64.0, 0.25);
        assert_eq!(s, 0.25);
        // Near the floor it slows: sqrt(2 × 0.1 × 0.01).
        let (s, arrived) = movement(10, 9.9, 64.0, 0.25);
        assert!((s - 0.044_721).abs() < 1e-5 && arrived, "{s}");
        // Down or up: the speed's sign doesn't matter.
        assert_eq!(
            movement(10, 0.0, -64.0, 0.0).0,
            movement(10, 0.0, 64.0, 0.0).0
        );
    }

    #[test]
    fn a_called_elevator_goes_to_its_floor_and_stops_level() {
        let mut e = elevator();
        // Assembled level with the floor at y 14 (the pulley's y + contact y - 1 - offset).
        assert_eq!(e.floor_now(20), 14);
        let start = e.floor_now(20);
        // Called down to the floor at y 3: it travels, arrives once, and settles level.
        let mut reached = Vec::new();
        let mut ticks = 0;
        while !e.settled(Some(3)) {
            let t = e.tick(20, 64.0, Some(3));
            reached.extend(t.reached);
            ticks += 1;
            assert!(ticks < 1000, "settles");
        }
        assert_eq!(reached, vec![3]);
        // Its contact level with the floor: the car's anchor at y 4, contact at 3.
        assert!((e.y - 4.0).abs() < 1e-6, "{}", e.y);
        assert_eq!(e.placed_at(), [0, 4, 0]);
        // And back up to where it started.
        let mut back = Vec::new();
        while !e.settled(Some(start)) {
            back.extend(e.tick(20, 64.0, Some(start)).reached);
        }
        assert_eq!(back, vec![start]);
        assert_eq!(e.placed_at(), [0, 15, 0]);
        // A floor out of its reach (above the pulley) is ignored: it stays.
        let before = e.clone();
        e.tick(20, 64.0, Some(40));
        assert_eq!(e.y, before.y);
    }

    #[test]
    fn contacts_signal_and_columns_keep_floors_sorted() {
        let mut c = Column::default();
        for y in [10, 3, 7, 3] {
            c.add(y);
        }
        assert_eq!(c.contacts, vec![3, 7, 10]);
        c.remove(7);
        assert_eq!(c.contacts, vec![3, 10]);
        assert!(!c.is_empty());
        assert_eq!(contact_signal(true, false), 15);
        assert_eq!(contact_signal(true, true), 0);
        assert_eq!(redstone_contact_signal(true, Dir::East, Dir::West), 0);
        assert_eq!(redstone_contact_signal(true, Dir::East, Dir::Up), 15);
        assert!(faces_back(Dir::East, Some(Dir::West)));
        assert_eq!(
            column_of([1, 5, 0], Dir::West),
            ColumnKey {
                x: 1,
                z: 0,
                side: Dir::West
            }
        );
    }

    #[test]
    fn floor_selection_follows_tick_floor_selection() {
        let floors = vec![
            Floor {
                y: 10,
                short: "0".into(),
                long: "Ground".into(),
            },
            Floor {
                y: 20,
                short: "1".into(),
                long: String::new(),
            },
        ];
        let mut i = 5;
        let s = select_floor(&mut i, &floors, 20, 10, 15);
        assert_eq!(i, 1, "clamped to the last floor");
        assert_eq!(s.short, "1");
        assert!(s.matches_target);
        assert!(s.unreachable, "above the highest contact it can reach");
        let mut i = -1;
        let s = select_floor(&mut i, &floors, 20, 10, 20);
        assert_eq!((i, s.target_y, s.matches_target), (0, 10, false));
        let mut i = 3;
        let s = select_floor(&mut i, &[], 0, 0, 0);
        assert_eq!(
            (i, s.short.as_str(), s.long.as_str()),
            (0, "X", "No Floors")
        );
        assert_eq!(floor_index(&floors, 20), Some(1));
        assert_eq!(floor_index(&floors, 30), None);
    }

    #[test]
    fn indicator_turns_toward_the_called_floor_and_settles() {
        let mut a = ControlsAnim::default();
        // Called up from y 0 to y 10: it starts at -181 and steps back, slowly through the
        // eighth it starts in (stage 3), then an eighth a step.
        a.tick_elevator(10, 0);
        assert_eq!(a.indicator.value, -181.0);
        a.tick_elevator(10, 0);
        assert_eq!(a.indicator.target, -181.0 - 2.25);
        for _ in 0..20 {
            a.tick_elevator(10, 0);
        }
        assert_eq!(a.indicator.target, -181.0 - 2.25 * 20.0 - 33.75);
        // Arrived: it runs on until it is back at a whole turn, then rests at 0.
        for _ in 0..40 {
            a.tick_elevator(10, 10);
        }
        assert_eq!(a.indicator.target, 0.0);
        assert_eq!(a.indicator_part(1.0), 0);
        // The button pressed comes back out (EXP at 1/8 a tick).
        a.press();
        assert_eq!(a.button.at(1.0), 1.0);
        a.tick();
        assert_eq!(a.button.value, 0.875);
    }
}
