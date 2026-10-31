//! Create's depot (`DepotBehaviour`, `SharedDepotBlockMethods`, `DepotRenderer`): one held stack
//! that slides to the centre when put down, and eight output slots processing fills. The stacks
//! live in the block entity's slots (0 held, 1 to 8 outputs); this keeps the held stack's motion
//! and draws them. A depot whose stack has arrived does nothing per tick.

use glam::{Mat4, Vec3};

/// The block entity's slots: the held stack, then the processing outputs.
pub const SLOTS: usize = 9;
pub const OUTPUTS: std::ops::Range<usize> = 1..9;

/// `java.util.Random`, for Create's seeded render offsets (`new Random(i + 1)`).
#[derive(Clone, Debug)]
pub struct JavaRandom(u64);

impl JavaRandom {
    const MULT: u64 = 0x5_DEEC_E66D;
    const MASK: u64 = (1 << 48) - 1;
    pub fn new(seed: i64) -> JavaRandom {
        JavaRandom((seed as u64 ^ Self::MULT) & Self::MASK)
    }
    fn next(&mut self, bits: u32) -> i32 {
        self.0 = (self.0.wrapping_mul(Self::MULT).wrapping_add(0xB)) & Self::MASK;
        (self.0 >> (48 - bits)) as i64 as i32
    }
    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 / (1 << 24) as f32
    }
}

/// The four horizontal faces, as `Direction` names them (index 0 north, 1 east, 2 south,
/// 3 west, clockwise seen from above).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    North,
    East,
    South,
    West,
}

impl Side {
    fn normal(self) -> Vec3 {
        match self {
            Side::North => Vec3::new(0.0, 0.0, -1.0),
            Side::East => Vec3::new(1.0, 0.0, 0.0),
            Side::South => Vec3::new(0.0, 0.0, 1.0),
            Side::West => Vec3::new(-1.0, 0.0, 0.0),
        }
    }
    pub fn index(self) -> u8 {
        self as u8
    }
    /// A horizontal `Direction` as a side.
    pub fn from_dir(d: crate::kinetics::Dir) -> Option<Side> {
        use crate::kinetics::Dir;
        match d {
            Dir::North => Some(Side::North),
            Dir::East => Some(Side::East),
            Dir::South => Some(Side::South),
            Dir::West => Some(Side::West),
            _ => None,
        }
    }
    pub fn from_index(i: u8) -> Option<Side> {
        [Side::North, Side::East, Side::South, Side::West]
            .get(usize::from(i))
            .copied()
    }
    /// The way a player at `yaw` (degrees, Minecraft's: 0 looks south) faces
    /// (`Entity.getDirection`).
    pub fn of_yaw(yaw: f32) -> Side {
        // Direction.fromYRot: floor(yaw / 90 + 0.5) & 3 over south, west, north, east.
        match ((yaw / 90.0 + 0.5).floor() as i32) & 3 {
            0 => Side::South,
            1 => Side::West,
            2 => Side::North,
            _ => Side::East,
        }
    }
}

/// `TransportedItemStack`'s motion on a depot: how far it has come (`beltPosition`, 0.5 is the
/// centre), the side it came from (`insertedFrom`; None from above), its angle, side offset, and
/// whether a machine holds it (`locked`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Held {
    pub position: f32,
    pub from: Option<Side>,
    pub angle: i32,
    pub side_offset: f32,
    pub locked: bool,
}

impl Held {
    /// `new TransportedItemStack(stack)`: a random angle (180 for upright items), the side
    /// offset that follows from it.
    pub fn new(angle: i32, upright: bool) -> Held {
        let angle = if upright { 180 } else { angle };
        Held {
            position: 0.5,
            from: None,
            angle,
            side_offset: (angle - 180) as f32 / (360.0 * 3.0),
            locked: false,
        }
    }

    /// `SharedDepotBlockMethods.onUse`: put down by a player facing `facing`, a quarter in.
    pub fn put_by_player(angle: i32, upright: bool, facing: Side) -> Held {
        Held {
            position: 0.25,
            from: Some(facing),
            ..Held::new(angle, upright)
        }
    }

    /// `DepotBehaviour.tick(TransportedItemStack)`: a quarter of the way left each tick, turning
    /// a degree while far off; true once it is within 1/16 of the centre.
    pub fn slide(&mut self, upright: bool) -> bool {
        let diff = 0.5 - self.position;
        if diff > 1.0 / 512.0 {
            if diff > 1.0 / 32.0 && !upright {
                self.angle += 1;
            }
            self.position += diff / 4.0;
        }
        diff < 1.0 / 16.0
    }

    /// Whether it still moves (a depot holding it ticks only then).
    pub fn sliding(&self) -> bool {
        0.5 - self.position > 1.0 / 512.0
    }

    /// Where `DepotRenderer` draws it, from the block's top centre: towards the centre from the
    /// side it came in through, and its side offset across.
    pub fn offset(&self) -> Vec3 {
        let Some(from) = self.from else {
            return Vec3::ZERO;
        };
        let along = -from.normal() * (0.5 - self.position);
        // insertedFrom.getClockWise(): north → east, east → south...
        let clockwise = Side::from_index((from.index() + 1) % 4).unwrap();
        let along_x = matches!(clockwise, Side::East | Side::West);
        let side = if along_x {
            Vec3::new(self.side_offset, 0.0, 0.0)
        } else {
            Vec3::new(0.0, 0.0, -self.side_offset)
        };
        along + side
    }
}

/// `DepotRenderer.renderItem`'s copies of a stack: one more for each two doublings of its count.
pub fn copies(count: u32) -> u32 {
    // Mth.log2: floor(log2(count)).
    (31 - count.max(1).leading_zeros()) / 2 + 1
}

/// `DepotRenderer.renderItem` without its turn (`rotateYDegrees(angle)`, drawn on the GPU as
/// the part's phase): the poses of a stack's copies under the depot's top centre
/// (`translate(.5, 15/16, .5)`), in block units. `block_item` is `isGui3d`; `rng` is the
/// renderer's `Random` (`new Random(0)` for the held stack, `i + 1` for output `i`).
pub fn stack_poses(count: u32, block_item: bool, rng: &mut JavaRandom) -> Vec<Mat4> {
    let mut out = Vec::new();
    let mut m = Mat4::IDENTITY;
    for i in 0..copies(count) {
        let mut c = m;
        if block_item {
            let (dx, dz) = (rng.next_float(), rng.next_float());
            c *= Mat4::from_translation(Vec3::new(
                dx * 0.0625 * i as f32,
                0.0,
                dz * 0.0625 * i as f32,
            ));
        }
        c *= Mat4::from_scale(Vec3::splat(0.5));
        if !block_item {
            c *= Mat4::from_translation(Vec3::new(0.0, -3.0 / 16.0, 0.0));
            c *= Mat4::from_rotation_x(90f32.to_radians());
        }
        out.push(c);
        if !block_item {
            m *= Mat4::from_rotation_y(10f32.to_radians());
        }
        m *= Mat4::from_translation(Vec3::new(
            0.0,
            if block_item { 1.0 / 64.0 } else { 1.0 / 16.0 },
            0.0,
        ));
    }
    out
}

/// The depot's top centre in its block (`translate(.5f, 15 / 16f, .5f)`).
pub fn top_centre() -> Mat4 {
    Mat4::from_translation(Vec3::new(0.5, 15.0 / 16.0, 0.5))
}

/// Output `i`'s (0 to 7) place: turned by 45° × i about the centre, 0.35 out, at its own seeded
/// angle. Returns the pose under the turn and the turn (the part's phase).
pub fn output_place(i: usize, count: u32, block_item: bool) -> (Vec<Mat4>, f32) {
    let mut rng = JavaRandom::new(i as i64 + 1);
    let angle = (360.0 * rng.next_float()) as i32;
    let out = Mat4::from_translation(Vec3::new(0.35, 0.0, 0.0))
        * Mat4::from_rotation_y((angle as f32).to_radians());
    let poses = stack_poses(count, block_item, &mut rng)
        .into_iter()
        .map(|p| out * p)
        .collect();
    (poses, 45.0 * i as f32)
}

/// What a player's use of a depot's top does (`SharedDepotBlockMethods.onUse`): every stack it
/// holds goes to the player, then the one in hand (if any) is put down.
#[derive(Debug, PartialEq)]
pub struct Use {
    pub to_player: Vec<usize>,
    pub put_down: bool,
}

pub fn use_top(slots_full: impl Fn(usize) -> bool, hand_empty: bool) -> Use {
    Use {
        to_player: (0..SLOTS).filter(|&s| slots_full(s)).collect(),
        put_down: !hand_empty,
    }
}

/// The mod-message channel that brings depots' contents to clients.
pub const CHANNEL: &str = "create:depot";

/// A depot's contents as clients draw them: the held stack's motion and every stack by slot
/// (none: the depot is empty or gone).
#[derive(Clone, Debug, PartialEq)]
pub struct Contents {
    pub pos: [i32; 3],
    pub held: Held,
    pub stacks: Vec<(u8, String, u8)>,
}

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn encode_one(c: &Contents, out: &mut Vec<u8>) {
    for v in c.pos {
        put_i32(out, v);
    }
    out.push(c.held.from.map_or(255, Side::index));
    out.push(u8::from(c.held.locked));
    put_i32(out, c.held.angle);
    out.extend_from_slice(&c.held.position.to_le_bytes());
    out.extend_from_slice(&c.held.side_offset.to_le_bytes());
    out.push(c.stacks.len() as u8);
    for (slot, item, count) in &c.stacks {
        let name = &item.as_bytes()[..item.len().min(255)];
        out.extend_from_slice(&[*slot, *count, name.len() as u8]);
        out.extend_from_slice(name);
    }
}

/// Messages of at most 30,000 bytes (a mod message's limit), whole depots in each.
pub fn encode(all: &[Contents]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut one = Vec::new();
    for c in all {
        one.clear();
        encode_one(c, &mut one);
        match out.last_mut() {
            Some(m) if m.len() + one.len() <= 30_000 => m.extend_from_slice(&one),
            _ => out.push(one.clone()),
        }
    }
    out
}

/// What a message holds; a malformed tail is dropped.
pub fn decode(mut d: &[u8]) -> Vec<Contents> {
    let mut out = Vec::new();
    let take = |d: &mut &[u8], n: usize| -> Option<Vec<u8>> {
        let (a, b) = (d.get(..n)?.to_vec(), d.get(n..)?);
        *d = b;
        Some(a)
    };
    let i32_of = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    let f32_of = |b: &[u8]| f32::from_le_bytes(b.try_into().unwrap());
    while !d.is_empty() {
        let Some(head) = take(&mut d, 12 + 2 + 12 + 1) else {
            break;
        };
        let mut c = Contents {
            pos: [
                i32_of(&head[0..4]),
                i32_of(&head[4..8]),
                i32_of(&head[8..12]),
            ],
            held: Held {
                from: Side::from_index(head[12]),
                locked: head[13] != 0,
                angle: i32_of(&head[14..18]),
                position: f32_of(&head[18..22]),
                side_offset: f32_of(&head[22..26]),
            },
            stacks: Vec::new(),
        };
        for _ in 0..head[26] {
            let Some(s) = take(&mut d, 3) else {
                return out;
            };
            let Some(name) = take(&mut d, usize::from(s[2])) else {
                return out;
            };
            c.stacks
                .push((s[0], String::from_utf8_lossy(&name).into_owned(), s[1]));
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_random_matches_the_jdk() {
        // new java.util.Random(1).nextFloat() and new Random(0).nextFloat(), from the JDK.
        assert_eq!(JavaRandom::new(1).next_float(), 0.7308782);
        assert_eq!(JavaRandom::new(0).next_float(), 0.73096776);
    }

    #[test]
    fn a_stack_put_down_slides_to_the_centre_in_five_ticks() {
        let mut h = Held::put_by_player(90, false, Side::North);
        let mut ticks = 0;
        while !h.slide(false) {
            ticks += 1;
        }
        // 0.25 · 0.75^n < 1/16 first at n = 5; the tick that sees it says so.
        assert_eq!(ticks, 5);
        assert!(h.sliding(), "it keeps creeping until within 1/512");
        // Every call while more than 1/32 away turns a degree, the arriving one too.
        assert_eq!(h.angle, 90 + 6);
        let mut more = 0;
        while h.sliding() {
            h.slide(false);
            more += 1;
        }
        assert!(more < 20);
        assert!(h.offset().z.abs() < 1.0 / 512.0 + 1e-6);
    }

    #[test]
    fn its_offset_runs_from_the_side_it_came_in_through() {
        // A player facing north put it down: it came from the north edge... insertedFrom north,
        // so it is drawn toward the south of the centre (offsetVec = south · (0.5 - 0.25)).
        let h = Held {
            side_offset: 0.0,
            ..Held::put_by_player(180, false, Side::North)
        };
        assert_eq!(h.offset(), Vec3::new(0.0, 0.0, 0.25));
        let h = Held {
            side_offset: 0.1,
            position: 0.5,
            ..Held::put_by_player(0, false, Side::North)
        };
        // north.getClockWise() is east: along x, not negated.
        assert_eq!(h.offset(), Vec3::new(0.1, 0.0, 0.0));
    }

    #[test]
    fn copies_grow_with_two_doublings() {
        assert_eq!([1, 3, 4, 15, 16, 64].map(copies), [1, 1, 2, 2, 3, 4]);
    }

    #[test]
    fn flat_items_lie_down_and_block_items_stand() {
        let flat = stack_poses(1, false, &mut JavaRandom::new(0));
        // Scaled by half, laid on its back: +y of the item points to -z... its up is now z.
        let up = flat[0].transform_vector3(Vec3::Y);
        assert!((up - Vec3::new(0.0, 0.0, 0.5)).length() < 1e-6, "{up}");
        let block = stack_poses(1, true, &mut JavaRandom::new(0));
        assert!((block[0].transform_vector3(Vec3::Y) - Vec3::new(0.0, 0.5, 0.0)).length() < 1e-6);
        assert_eq!(stack_poses(64, true, &mut JavaRandom::new(0)).len(), 4);
    }

    #[test]
    fn using_the_top_gives_back_everything_then_puts_down() {
        let u = use_top(|s| s == 0 || s == 3, false);
        assert_eq!(
            u,
            Use {
                to_player: vec![0, 3],
                put_down: true
            }
        );
        assert!(!use_top(|_| false, true).put_down);
    }

    #[test]
    fn contents_round_trip_in_messages() {
        let one = Contents {
            pos: [-5, 70, 1_000_000],
            held: Held::put_by_player(33, false, Side::West),
            stacks: vec![
                (0, "minecraft:iron_ingot".into(), 64),
                (4, "create:iron_sheet".into(), 3),
            ],
        };
        let empty = Contents {
            pos: [1, 2, 3],
            held: Held::new(0, false),
            stacks: vec![],
        };
        let messages = encode(&[one.clone(), empty.clone()]);
        assert_eq!(messages.len(), 1);
        assert_eq!(decode(&messages[0]), vec![one.clone(), empty]);
        let many: Vec<Contents> = (0..2000).map(|_| one.clone()).collect();
        let messages = encode(&many);
        assert!(messages.len() > 1 && messages.iter().all(|m| m.len() <= 30_000));
        assert_eq!(
            messages.iter().map(|m| decode(m).len()).sum::<usize>(),
            2000
        );
    }
}
