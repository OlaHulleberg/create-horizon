//! What a moving structure is to the entities around it (`ContraptionCollider.collideEntities`):
//! its blocks' collision boxes joined into few (`Contraption.getSimplifiedEntityColliders`), and
//! where they stand each tick.
//!
//! Every structure is posed the same way, as its actors work and its clients draw it: the centre
//! of its block `[0, 0, 0]` at a point of the world, its blocks turned about that centre by up to
//! three turns (`actors::Turns`).

use crate::actors::{NO_TURNS, Turns, one_turn, turn_all, unturn};
use crate::kinetics::Axis;

/// An axis-aligned box: `[min x, min y, min z, max x, max y, max z]`.
pub type Aabb = [f64; 6];

/// Where a structure stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// The centre of its block `[0, 0, 0]` in the world.
    pub centre: [f64; 3],
    pub turns: Turns,
}

impl Pose {
    /// A structure moved without turning, its block `[0, 0, 0]` with its lower corner at `corner`.
    pub fn translated(corner: [f64; 3]) -> Pose {
        Pose {
            centre: corner.map(|v| v + 0.5),
            turns: NO_TURNS,
        }
    }

    /// A structure whose block offsets are from `anchor`, turned `deg` about `axis` through the
    /// anchor's centre (a bearing's).
    pub fn turned(anchor: [i32; 3], axis: Axis, deg: f64) -> Pose {
        Pose {
            centre: anchor.map(|v| f64::from(v) + 0.5),
            turns: one_turn(axis, deg),
        }
    }

    /// `toGlobalVector`: a point of the structure, in its blocks' space, in the world.
    pub fn to_world(&self, local: [f64; 3]) -> [f64; 3] {
        let turned = turn_all(local.map(|v| v - 0.5), &self.turns);
        [0, 1, 2].map(|i| self.centre[i] + turned[i])
    }

    /// Its turns as one unit quaternion `[x, y, z, w]`.
    pub fn rotation(&self) -> [f64; 4] {
        self.turns
            .iter()
            .fold([0.0, 0.0, 0.0, 1.0], |q, &(axis, deg)| {
                multiply(about(axis, deg), q)
            })
    }

    /// Where the boxes of [`simplified`] stand for an entity at `at` carrying them turned by
    /// [`Pose::rotation`]: how far their frame is shifted from the entity, in the frame before it
    /// turns.
    pub fn shift_from(&self, at: [f64; 3]) -> [f64; 3] {
        unturn([0, 1, 2].map(|i| self.centre[i] - at[i]), &self.turns)
    }
}

/// Where something that stood at `before` (a point and a unit quaternion) a tick ago and stands
/// at `now` will be a tick on, going on as it went.
pub fn ahead(before: ([f64; 3], [f64; 4]), now: ([f64; 3], [f64; 4])) -> ([f64; 3], [f64; 4]) {
    let at = [0, 1, 2].map(|i| 2.0 * now.0[i] - before.0[i]);
    let [x, y, z, w] = before.1;
    let step = multiply(now.1, [-x, -y, -z, w]);
    (at, multiply(step, now.1))
}

/// `deg` degrees about `axis`, counterclockwise looking down the positive axis (`turn_vec`'s
/// sense), as a unit quaternion.
fn about(axis: Axis, deg: f64) -> [f64; 4] {
    let (s, c) = (deg.to_radians() * 0.5).sin_cos();
    match axis {
        Axis::X => [s, 0.0, 0.0, c],
        Axis::Y => [0.0, s, 0.0, c],
        Axis::Z => [0.0, 0.0, s, c],
    }
}

/// `a × b`: `b`'s turn, then `a`'s.
fn multiply(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// `Contraption.getSimplifiedEntityColliders` (`gatherBBsOffThread`: the union of the blocks'
/// collision shapes, `optimize().toAabbs()`): each block's boxes (block-local) moved by its
/// offset, relative to the centre of block `[0, 0, 0]`, then boxes that meet face to face with
/// the same cross-section joined along x, then z, then y. The union stays exactly the same.
pub fn simplified<'a>(blocks: impl IntoIterator<Item = ([i32; 3], &'a [Aabb])>) -> Vec<Aabb> {
    let mut boxes: Vec<Aabb> = blocks
        .into_iter()
        .flat_map(|(o, shape)| {
            let o = o.map(|v| f64::from(v) - 0.5);
            shape.iter().map(move |b| {
                [
                    b[0] + o[0],
                    b[1] + o[1],
                    b[2] + o[2],
                    b[3] + o[0],
                    b[4] + o[1],
                    b[5] + o[2],
                ]
            })
        })
        .filter(|b| b[0] < b[3] && b[1] < b[4] && b[2] < b[5])
        .collect();
    for axis in [0, 2, 1] {
        boxes = join_along(boxes, axis);
    }
    boxes
}

/// Joins the boxes that touch or overlap along `axis` and match on the other two axes.
fn join_along(mut boxes: Vec<Aabb>, axis: usize) -> Vec<Aabb> {
    let across: Vec<usize> = (0..3).filter(|&a| a != axis).collect();
    let key = |b: &Aabb| {
        across
            .iter()
            .flat_map(|&a| [b[a].to_bits(), b[a + 3].to_bits()])
            .collect::<Vec<u64>>()
    };
    boxes.sort_by(|a, b| key(a).cmp(&key(b)).then(a[axis].total_cmp(&b[axis])));
    let mut out: Vec<Aabb> = Vec::with_capacity(boxes.len());
    for b in boxes {
        if let Some(last) = out.last_mut()
            && key(last) == key(&b)
            && b[axis] <= last[axis + 3]
        {
            last[axis + 3] = last[axis + 3].max(b[axis + 3]);
            continue;
        }
        out.push(b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE: [Aabb; 1] = [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
    const SLAB: [Aabb; 1] = [[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]];

    fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
        let p = [v[0], v[1], v[2], 0.0];
        let inv = [-q[0], -q[1], -q[2], q[3]];
        let r = multiply(multiply(q, p), inv);
        [r[0], r[1], r[2]]
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
    }

    #[test]
    fn a_plane_of_cubes_joins_into_one_box() {
        let blocks: Vec<([i32; 3], &[Aabb])> = (0..4)
            .flat_map(|x| (0..3).map(move |z| ([x, 0, z], &CUBE[..])))
            .collect();
        assert_eq!(simplified(blocks), vec![[-0.5, -0.5, -0.5, 3.5, 0.5, 2.5]]);
    }

    #[test]
    fn boxes_of_other_sections_stay_apart() {
        let blocks: Vec<([i32; 3], &[Aabb])> = vec![
            ([0, 0, 0], &CUBE[..]),
            ([1, 0, 0], &SLAB[..]),
            ([2, 0, 0], &SLAB[..]),
            ([0, 2, 0], &CUBE[..]),
        ];
        let mut got = simplified(blocks);
        got.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(
            got,
            vec![
                [-0.5, -0.5, -0.5, 0.5, 0.5, 0.5],
                [-0.5, 1.5, -0.5, 0.5, 2.5, 0.5],
                [0.5, -0.5, -0.5, 2.5, 0.0, 0.5],
            ]
        );
    }

    #[test]
    fn a_block_without_collision_adds_nothing() {
        assert!(simplified([([3, 1, 2], &[][..])]).is_empty());
        assert!(simplified([([0, 0, 0], &[[0.0, 0.0, 0.0, 1.0, 0.0, 1.0]][..])]).is_empty());
    }

    #[test]
    fn the_rotation_turns_as_the_actors_and_clients_turn() {
        let turns = [(Axis::Y, 30.0), (Axis::X, -12.0), (Axis::Y, 75.0)];
        let pose = Pose {
            centre: [10.5, 64.5, -3.5],
            turns,
        };
        let q = pose.rotation();
        assert!((q.iter().map(|v| v * v).sum::<f64>() - 1.0).abs() < 1e-12);
        for v in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.3, -2.0, 5.0]] {
            assert!(close(rotate(q, v), turn_all(v, &turns)));
        }
    }

    #[test]
    fn a_bearing_turns_its_blocks_about_its_centre() {
        let pose = Pose::turned([0, 64, 0], Axis::Y, 90.0);
        // The block east of the bearing has gone north.
        assert!(close(pose.to_world([1.5, 0.5, 0.5]), [0.5, 64.5, -0.5]));
        assert!(close(pose.shift_from(pose.centre), [0.0; 3]));
    }

    #[test]
    fn a_frame_carried_by_another_entity_is_shifted_back_into_its_own_turn() {
        let pose = Pose {
            centre: [0.0, 1.5, 0.0],
            turns: one_turn(Axis::X, 90.0),
        };
        let shift = pose.shift_from([0.0, 1.0, 0.0]);
        let back = rotate(pose.rotation(), shift);
        assert!(close(back, [0.0, 0.5, 0.0]));
    }

    #[test]
    fn a_structure_goes_on_as_it_went() {
        let pose = |deg, y| Pose {
            centre: [0.5, y, 0.5],
            turns: one_turn(Axis::Y, deg),
        };
        let (a, b, c) = (pose(10.0, 64.0), pose(14.8, 64.25), pose(19.6, 64.5));
        let (at, q) = ahead((a.centre, a.rotation()), (b.centre, b.rotation()));
        assert!(close(at, c.centre));
        let expected = c.rotation();
        assert!((0..4).all(|i| (q[i] - expected[i]).abs() < 1e-9));
    }

    #[test]
    fn a_translated_structure_stands_at_its_corner() {
        let pose = Pose::translated([3.0, 70.25, -8.0]);
        assert_eq!(pose.rotation(), [0.0, 0.0, 0.0, 1.0]);
        assert!(close(pose.to_world([0.0; 3]), [3.0, 70.25, -8.0]));
    }
}
